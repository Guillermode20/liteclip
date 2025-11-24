using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Routing;
using liteclip.Models;
using liteclip.Services;

namespace liteclip.Endpoints;

public static class SettingsEndpoints
{
    public static IEndpointRouteBuilder MapSettingsEndpoints(this IEndpointRouteBuilder endpoints)
    {
        endpoints.MapGet("/api/settings", async (UserSettingsStore store) =>
            {
                var settings = await store.GetAsync();
                return Results.Ok(settings);
            })
            .WithName("GetUserSettings");

        endpoints.MapPost("/api/settings", async ([FromBody] UserSettings settings, UserSettingsStore store) =>
            {
                var updated = await store.UpdateAsync(settings);
                return Results.Ok(updated);
            })
            .WithName("UpdateUserSettings");

        return endpoints;
    }
}
