using System;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Windows.Forms;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;

namespace smart_compressor;

[SupportedOSPlatform("windows6.1")]
public class WebViewWindow : Form
{
    private WebView2 webView;
    private CoreWebView2Environment? webViewEnvironment;

    public WebViewWindow(string url, string title = "Smart Video Compressor")
    {
        // Configure the form with Windows theme support
        Text = title;
        Width = 1200;
        Height = 800;
        StartPosition = FormStartPosition.CenterScreen;
        
        // Enable dark mode for title bar on Windows 10/11
        ApplyWindowsTheme();

        // Create and configure WebView2
        webView = new WebView2
        {
            Dock = DockStyle.Fill
        };

        Controls.Add(webView);

        // Initialize WebView2 environment and navigate
        InitializeWebView(url);

        // Handle window closing
        FormClosing += (sender, e) =>
        {
            Console.WriteLine("Window closing...");
        };
    }

    private void ApplyWindowsTheme()
    {
        try
        {
            // Check if dark mode is enabled in Windows
            bool isDarkMode = IsSystemDarkMode();
            
            if (isDarkMode && OperatingSystem.IsWindowsVersionAtLeast(10, 0, 17763))
            {
                // Apply dark mode to title bar (Windows 10 1809+ / Windows 11)
                int useImmersiveDarkMode = 1;
                DwmSetWindowAttribute(Handle, DWMWA_USE_IMMERSIVE_DARK_MODE, ref useImmersiveDarkMode, sizeof(int));
                
                // Set form colors to match dark theme
                BackColor = System.Drawing.Color.FromArgb(32, 32, 32);
            }
            else
            {
                // Light mode
                BackColor = System.Drawing.SystemColors.Control;
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"⚠️ Could not apply Windows theme: {ex.Message}");
        }
    }

    private async void InitializeWebView(string url)
    {
        try
        {
            // Create WebView2 environment with optimized settings
            var environmentOptions = new CoreWebView2EnvironmentOptions
            {
                AdditionalBrowserArguments = "--disable-web-security --disable-features=IsolateOrigins --disable-site-isolation-trials"
            };
            
            webViewEnvironment = await CoreWebView2Environment.CreateAsync(
                null, 
                Path.Combine(Path.GetTempPath(), "SmartCompressor_WebView2"), 
                environmentOptions
            );
            
            await webView.EnsureCoreWebView2Async(webViewEnvironment);
            
            // Enable dev tools (F12)
            webView.CoreWebView2.Settings.AreDevToolsEnabled = true;
            webView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = true;
            
            // Sync WebView2 theme with Windows theme
            bool isDarkMode = IsSystemDarkMode();
            webView.CoreWebView2.Profile.PreferredColorScheme = isDarkMode 
                ? CoreWebView2PreferredColorScheme.Dark 
                : CoreWebView2PreferredColorScheme.Light;
            
            // Navigate to the URL
            webView.CoreWebView2.Navigate(url);
            
            Console.WriteLine($"✓ WebView2 initialized with {(isDarkMode ? "dark" : "light")} theme and navigated to: {url}");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"❌ Error initializing WebView2: {ex.Message}");
            MessageBox.Show($"Error loading application:\n{ex.Message}", "Error", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private bool IsSystemDarkMode()
    {
        try
        {
            using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            
            if (key?.GetValue("AppsUseLightTheme") is int value)
            {
                return value == 0; // 0 = dark mode, 1 = light mode
            }
        }
        catch
        {
            // If we can't read the registry, assume light mode
        }
        
        return false;
    }

    // Windows API for dark mode title bar
    private const int DWMWA_USE_IMMERSIVE_DARK_MODE = 20;

    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int attrValue, int attrSize);

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        base.OnFormClosed(e);
        webView?.Dispose();
    }
}

