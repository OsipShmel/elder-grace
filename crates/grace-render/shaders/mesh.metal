// Simple mesh: positions and normals in separate buffers, Lambert + hemispherical light.
#include <metal_stdlib>
using namespace metal;

struct Uniforms {
    float4x4 view_proj;
    float4 light;   // xyz = direction towards the light (world space)
    float4 color;   // mesh color without a texture (linear)
    float4 params;  // x = 1 if there is a texture
};

struct VOut {
    float4 pos [[position]];
    float3 normal;
    float2 uv;
};

vertex VOut grace_mesh_vs(uint vid [[vertex_id]],
                          const device packed_float3* positions [[buffer(0)]],
                          const device packed_float3* normals [[buffer(1)]],
                          const device packed_float2* uvs [[buffer(2)]],
                          constant Uniforms& u [[buffer(3)]],
                          const device float4x4* instances [[buffer(4)]],
                          uint iid [[instance_id]]) {
    VOut o;
    float4x4 m = instances[iid];
    o.pos = u.view_proj * (m * float4(float3(positions[vid]), 1.0));
    o.normal = (m * float4(float3(normals[vid]), 0.0)).xyz;
    o.uv = float2(uvs[vid]);
    return o;
}

fragment float4 grace_mesh_fs(VOut in [[stage_in]],
                              constant Uniforms& u [[buffer(0)]],
                              texture2d<float> albedo [[texture(0)]],
                              sampler smp [[sampler(0)]],
                              bool front [[front_facing]]) {
    float3 n = normalize(in.normal) * (front ? 1.0 : -1.0);
    float4 base = u.color;
    if (u.params.x > 0.5) {
        base = albedo.sample(smp, in.uv);
        if (base.a < 0.5) discard_fragment();   // foliage, grates: alpha test
    }
    float direct = max(dot(n, normalize(u.light.xyz)), 0.0);
    float sky = 0.5 + 0.5 * n.y;
    float3 c = base.rgb * (0.25 + 0.35 * sky + 0.75 * direct);
    return float4(c, 1.0);
}
