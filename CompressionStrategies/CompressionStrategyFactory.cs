using System.Collections.Generic;
using System.Linq;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Default implementation of <see cref="ICompressionStrategyFactory"/> which
/// resolves registered strategies by their codec key.
/// </summary>
public class CompressionStrategyFactory : ICompressionStrategyFactory
{
    private readonly Dictionary<string, ICompressionStrategy> _strategies;

    public CompressionStrategyFactory(IEnumerable<ICompressionStrategy> strategies)
    {
        _strategies = strategies
            .Where(s => s != null && !string.IsNullOrWhiteSpace(s.CodecKey))
            .ToDictionary(s => s.CodecKey.ToLowerInvariant(), s => s);
    }

    public ICompressionStrategy? GetStrategy(string codecKey)
    {
        if (string.IsNullOrWhiteSpace(codecKey)) return null;
        _strategies.TryGetValue(codecKey.ToLowerInvariant(), out var strategy);
        return strategy;
    }
}
