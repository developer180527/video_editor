// 0 amount (px): the picture fades out towards its edges.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let edge = min(i.uv, vec2f(1.0) - i.uv) * texels() / params.scale;
    let f = max(params.values[0].x, 1e-4);
    return src(i.uv) * smoothstep(0.0, f, min(edge.x, edge.y));
}
