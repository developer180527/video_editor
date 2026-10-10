// 0 amount (%), 1 radius (px), 2 threshold (0..255).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let blurred = straight(blur2(source, i.uv, params.values[1].x * params.scale, false));
    let e = encode(s.rgb);
    let diff = e - encode(blurred.rgb);
    let gate = step(params.values[2].x / 255.0, abs(luma(diff)));
    let o = e + diff * (params.values[0].x / 100.0) * gate;
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
