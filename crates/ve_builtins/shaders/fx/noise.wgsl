// 0 amount (%), 1 colour noise, 2 grain size (px), 3 animated.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let cell = floor(uv_px(i.uv) / max(params.values[2].x, 1.0));
    let seed = select(0.0, floor(params.time * 120.0), on(3u));
    let base = cell + vec2f(seed * 17.13, seed * 5.71);
    var n = vec3f(hash12(base));
    if (on(1u)) {
        n = vec3f(n.x, hash12(base + vec2f(91.7, 13.1)), hash12(base + vec2f(47.3, 77.9)));
    }
    let o = encode(s.rgb) + (n - 0.5) * params.values[0].x / 100.0;
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
