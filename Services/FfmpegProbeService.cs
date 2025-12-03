using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Text.RegularExpressions;
using System.Threading.Tasks;
using Microsoft.Extensions.Logging;
using liteclip.Models;

namespace liteclip.Services
{
    public class FfmpegProbeService
    {
        private readonly FfmpegPathResolver _pathResolver;
        private readonly ILogger<FfmpegProbeService> _logger;
        private readonly TimeSpan _cacheTtl = TimeSpan.FromMinutes(30);
        private DateTime _lastRefreshUtc = DateTime.MinValue;
        private List<FfmpegEncoderInfo>? _cachedEncoders;

        public FfmpegProbeService(FfmpegPathResolver pathResolver, ILogger<FfmpegProbeService> logger)
        {
            _pathResolver = pathResolver;
            _logger = logger;
        }

        /// <summary>
        /// Returns a short version string from the ffmpeg executable (the first non-empty line of stdout/stderr) or null if unavailable.
        /// </summary>
        public async Task<string?> GetFfmpegVersionAsync()
        {
            try
            {
                var ffmpegPath = _pathResolver.GetFfmpegPath();
                if (string.IsNullOrWhiteSpace(ffmpegPath)) return null;

                var psi = new ProcessStartInfo
                {
                    FileName = ffmpegPath,
                    Arguments = "-version",
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true
                };

                using var proc = Process.Start(psi);
                if (proc == null) return null;

                // Read the first non-empty line from stdout or stderr within a short timeout
                string? firstLine = null;
                var readStdOut = proc.StandardOutput.ReadLineAsync();
                var readStdErr = proc.StandardError.ReadLineAsync();

                var completed = await Task.WhenAny(readStdOut, readStdErr, Task.Delay(2000)).ConfigureAwait(false);
                if (completed == readStdOut)
                {
                    firstLine = await readStdOut.ConfigureAwait(false);
                }
                else if (completed == readStdErr)
                {
                    firstLine = await readStdErr.ConfigureAwait(false);
                }

                try
                {
                    // Ensure ffmpeg process exits or kill after timeout
                    await proc.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(2)).ConfigureAwait(false);
                }
                catch
                {
                    try { proc.Kill(); } catch { }
                }

                if (string.IsNullOrWhiteSpace(firstLine))
                {
                    // Fallback: read any buffered output
                    try
                    {
                        var outTxt = await proc.StandardOutput.ReadToEndAsync().ConfigureAwait(false);
                        if (!string.IsNullOrWhiteSpace(outTxt))
                        {
                            var lines = outTxt.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries);
                            firstLine = lines.FirstOrDefault();
                        }
                    }
                    catch { }
                }

                if (string.IsNullOrWhiteSpace(firstLine))
                {
                    try
                    {
                        var errTxt = await proc.StandardError.ReadToEndAsync().ConfigureAwait(false);
                        if (!string.IsNullOrWhiteSpace(errTxt))
                        {
                            var lines = errTxt.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries);
                            firstLine = lines.FirstOrDefault();
                        }
                    }
                    catch { }
                }

                if (!string.IsNullOrWhiteSpace(firstLine))
                {
                    return firstLine.Trim();
                }

                return null;
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "Failed to probe ffmpeg version");
                return null;
            }
        }

        public async Task<List<FfmpegEncoderInfo>> GetEncodersAsync(bool verify = false)
        {
            // Return cached value if within TTL and verification not requested
            if (!verify && _cachedEncoders != null && DateTime.UtcNow - _lastRefreshUtc < _cacheTtl)
            {
                return _cachedEncoders;
            }

            var ffmpegPath = _pathResolver.GetFfmpegPath();
            if (string.IsNullOrWhiteSpace(ffmpegPath))
            {
                throw new InvalidOperationException("FFmpeg executable not found");
            }

            try
            {
                var args = "-hide_banner -encoders";
                var psi = new ProcessStartInfo
                {
                    FileName = ffmpegPath,
                    Arguments = args,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true
                };

                using var proc = Process.Start(psi);
                if (proc == null)
                {
                    _logger.LogWarning("Failed to start ffmpeg process while probing encoders");
                    return new List<FfmpegEncoderInfo>();
                }

                var output = await proc.StandardOutput.ReadToEndAsync();
                var err = await proc.StandardError.ReadToEndAsync();
                proc.WaitForExit();

                if (proc.ExitCode != 0 && string.IsNullOrWhiteSpace(output))
                {
                    _logger.LogWarning("FFmpeg -encoders exited with code {Code}. StdErr: {Err}", proc.ExitCode, err);
                }

                var parsed = ParseEncodersFromOutput(output ?? err ?? string.Empty);

                // Only verify hardware encoders when explicitly requested via 'verify' to avoid heavy CPU workload
                if (verify)
                {
                    var hardwareEncoders = parsed.Where(e => e.IsHardware).ToList();
                    foreach (var e in hardwareEncoders)
                    {
                        e.IsAvailable = await IsEncoderAvailable(ffmpegPath, e.Name);
                    }
                }

                // Optionally run availability checks for software encoders when verify=true
                if (verify)
                {
                    var software = parsed.Where(e => !e.IsHardware).ToList();
                    foreach (var e in software)
                    {
                        e.IsAvailable = await IsEncoderAvailable(ffmpegPath, e.Name);
                    }
                }

                _cachedEncoders = parsed;
                _lastRefreshUtc = DateTime.UtcNow;
                return parsed;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error while probing ffmpeg encoders");
                return new List<FfmpegEncoderInfo>();
            }
        }

        // Encoders that are actually used in the app (strategies and codec configs)
        private static readonly string[] _allowedEncoders = new[]
        {
            // H.264 hardware encoders
            "h264_nvenc",
            "h264_qsv",
            "h264_videotoolbox",
            "h264_amf",
            "h264_vaapi",
            // H.265 hardware encoders
            "hevc_nvenc",
            "hevc_qsv",
            "hevc_videotoolbox",
            "hevc_amf",
            "hevc_vaapi",
            // Software-based encoders used by the app
            "libx264",
            "libx265",
            // Accept common alias names as well
            "x264",
            "hevc"
        };

        private static bool IsCriticalEncoder(string name)
        {
            var lower = name.ToLowerInvariant();
            return Array.Exists(_allowedEncoders, e => e.Equals(lower, StringComparison.OrdinalIgnoreCase));
        }

        private static List<FfmpegEncoderInfo> ParseEncodersFromOutput(string output)
        {
            var list = new List<FfmpegEncoderInfo>();

            if (string.IsNullOrWhiteSpace(output)) return list;

            var lines = output.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries);
            var encoderLineRegex = new Regex(@"^[\W\w]*?\s+(?<name>[A-Za-z0-9_\-:.]+)\s+(?<desc>.+)$", RegexOptions.Compiled);

            foreach (var raw in lines)
            {
                var line = raw.Trim();
                // Lines in 'ffmpeg -encoders' contain something like:
                // V..... h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
                // We only want the encoder name and description
                if (line.StartsWith("--") || line.StartsWith("Encoders:") || line.StartsWith("----") || line.StartsWith("V.....") || line.StartsWith("A....."))
                {
                    // avoid accidental match
                }

                // Skip heading lines that do not have the field
                if (line.StartsWith("Encoders:", StringComparison.OrdinalIgnoreCase) || line.StartsWith("------")) continue;

                // The ffmpeg output uses grouped columns - the name is after the flags; the name column is aligned
                // If line starts with a flag char (e.g. 'V..... ') then parse it
                if (line.Length < 6) continue;

                // If the line does not include a space after 6 chars, skip
                var afterFlags = line.Substring(6).TrimStart();
                var parts = afterFlags.Split(new[] { ' ' }, 2, StringSplitOptions.RemoveEmptyEntries);
                if (parts.Length == 0) continue;
                var name = parts[0].Trim();
                var desc = parts.Length > 1 ? parts[1].Trim() : null;

                if (string.IsNullOrWhiteSpace(name)) continue;

                var isHardware = name.ToLowerInvariant().Contains("nvenc") || name.ToLowerInvariant().Contains("qsv") || name.ToLowerInvariant().Contains("amf") || name.ToLowerInvariant().Contains("vaapi") || name.ToLowerInvariant().Contains("videotoolbox");

                list.Add(new FfmpegEncoderInfo
                {
                    Name = name,
                    Description = desc,
                    IsHardware = isHardware
                });
            }

            // Remove duplicates and sort by name to make predictable
                var result = list
                .GroupBy(e => e.Name)
                .Select(g => g.First())
                    .OrderBy(e => e.IsHardware ? 0 : 1) // hardware first
                .ThenBy(e => e.Name)
                .ToList();
                // Filter to only the encoders used by the app for a minimal UI
                result = result.Where(e => _allowedEncoders.Contains(e.Name, StringComparer.OrdinalIgnoreCase)).ToList();

            return result;
        }

        private async Task<bool> IsEncoderAvailable(string ffmpegPath, string encoderName)
        {
            try
            {
                var attempts = new[]
                {
                    // Use testsrc with NV12 which is a reasonable test for many encoders
                    $"-hide_banner -loglevel error -f lavfi -i testsrc=duration=0.25:size=640x360:rate=15 -pix_fmt nv12 -c:v {encoderName} -g 60 -b:v 500k -bf 0 -f null -",
                    // Fallback minimal test
                    $"-hide_banner -loglevel error -f lavfi -i color=black:s=64x64:d=0.12 -c:v {encoderName} -f null -"
                };

                foreach (var args in attempts)
                {
                    var psi = new ProcessStartInfo
                    {
                        FileName = ffmpegPath,
                        Arguments = args,
                        RedirectStandardOutput = true,
                        RedirectStandardError = true,
                        UseShellExecute = false,
                        CreateNoWindow = true
                    };

                    using var p = Process.Start(psi);
                    if (p == null) continue;
                    var err = p.StandardError.ReadToEndAsync();
                    // Wait with timeout (3s) to prevent hangs in probing
                    var exited = p.WaitForExit(3000);
                    if (!exited)
                    {
                        try { p.Kill(); } catch { }
                        return false;
                    }
                    var errStr = await err;
                    if (p.ExitCode == 0) return true;
                    var lerr = errStr.ToLowerInvariant();
                    if (lerr.Contains("not available") || lerr.Contains("cannot load") || lerr.Contains("no nvenc"))
                    {
                        return false;
                    }
                }

                return false;
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "Runtime check for encoder {Encoder} failed", encoderName);
                return false;
            }
        }
    }
}
