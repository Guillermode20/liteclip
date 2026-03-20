// Simple texture sampling pixel shader with bilinear filtering
// Samples from a BGRA texture and outputs the color

Texture2D tx : register(t0);
SamplerState smp : register(s0);

struct PS_INPUT {
    float4 pos : SV_Position;
    float2 tex : TEXCOORD;
};

float4 main(PS_INPUT input) : SV_Target {
    return tx.Sample(smp, input.tex);
}
