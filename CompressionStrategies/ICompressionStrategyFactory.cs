namespace liteclip.CompressionStrategies;

/// <summary>
/// Factory abstraction to obtain the appropriate <see cref="ICompressionStrategy"/> for a codec.
/// Implementations can use DI to register available strategies and select by codec key.
/// </summary>
public interface ICompressionStrategyFactory
{
    /// <summary>
    /// Returns the compression strategy for the specified codec key or null if not available.
    /// </summary>
    ICompressionStrategy? GetStrategy(string codecKey);
}
