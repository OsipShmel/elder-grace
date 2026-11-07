// Тестовый треугольник фазы 0: проверяем, что конвейер Metal живой.
#include <metal_stdlib>
using namespace metal;

struct VOut {
    float4 position [[position]];
    float3 color;
};

vertex VOut grace_vs(uint vid [[vertex_id]]) {
    const float2 p[3] = { float2(0.0, 0.6), float2(-0.6, -0.5), float2(0.6, -0.5) };
    const float3 c[3] = { float3(1.0, 0.3, 0.2), float3(0.2, 0.9, 0.4), float3(0.2, 0.4, 1.0) };
    VOut o;
    o.position = float4(p[vid], 0.0, 1.0);
    o.color = c[vid];
    return o;
}

fragment float4 grace_fs(VOut in [[stage_in]], constant float& time [[buffer(0)]]) {
    float k = 0.8 + 0.2 * sin(time * 2.0);
    return float4(in.color * k, 1.0);
}
