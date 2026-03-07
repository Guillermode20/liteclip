// Simple pass-through vertex shader for fullscreen quad
// Input: float2 position, float2 texcoord
// Output: float4 position (SV_Position), float2 texcoord

struct VS_INPUT {
    float2 pos : POSITION;
    float2 tex : TEXCOORD;
};

struct VS_OUTPUT {
    float4 pos : SV_Position;
    float2 tex : TEXCOORD;
};

VS_OUTPUT main(VS_INPUT input) {
    VS_OUTPUT output;
    output.pos = float4(input.pos, 0.0, 1.0);
    output.tex = input.tex;
    return output;
}