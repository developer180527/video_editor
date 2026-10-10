// 0 invert (dark background), 1 blend with original (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let t = 1.0 / texels();
    let s = straight(src(i.uv));
    var gx = vec3f(0.0);
    var gy = vec3f(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let c = encode(straight(src(i.uv + vec2f(f32(x), f32(y)) * t)).rgb);
            let wx = f32(x) * (2.0 - abs(f32(y)));
            let wy = f32(y) * (2.0 - abs(f32(x)));
            gx += wx * c;
            gy += wy * c;
        }
    }
    let edges = clamp(sqrt(gx * gx + gy * gy) * 0.5, vec3f(0.0), vec3f(1.0));
    let lines = select(vec3f(1.0) - edges, edges, on(0u));
    let o = mix(lines, encode(s.rgb), params.values[1].x / 100.0);
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
