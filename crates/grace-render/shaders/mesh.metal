// Простой меш: позиции и нормали в отдельных буферах, ламберт + полусферный свет.
#include <metal_stdlib>
using namespace metal;

struct Uniforms {
    float4x4 view_proj;
    float4 light;   // xyz — направление на свет (мировые координаты)
    float4 color;   // базовый цвет меша (линейный)
};

struct VOut {
    float4 pos [[position]];
    float3 normal;
};

vertex VOut grace_mesh_vs(uint vid [[vertex_id]],
                          const device packed_float3* positions [[buffer(0)]],
                          const device packed_float3* normals [[buffer(1)]],
                          constant Uniforms& u [[buffer(2)]]) {
    VOut o;
    o.pos = u.view_proj * float4(float3(positions[vid]), 1.0);
    o.normal = float3(normals[vid]);
    return o;
}

fragment float4 grace_mesh_fs(VOut in [[stage_in]], constant Uniforms& u [[buffer(0)]]) {
    float3 n = normalize(in.normal);
    float direct = max(dot(n, normalize(u.light.xyz)), 0.0);
    float sky = 0.5 + 0.5 * n.y;
    float3 c = u.color.rgb * (0.18 + 0.32 * sky + 0.7 * direct);
    return float4(c, 1.0);
}
