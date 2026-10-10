// 0 level (0..255), 1 softness (%): white above, black below.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let l = luma(encode(s.rgb));
    let t = params.values[0].x / 255.0;
    let w = params.values[1].x / 200.0;
    let v = select(step(t, l), smoothstep(t - w, t + w, l), w > 0.0);
    return premul(vec4f(vec3f(v), s.a));
}
