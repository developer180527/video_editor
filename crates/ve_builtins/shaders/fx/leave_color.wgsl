// 0 colour to leave, 1 tolerance, 2 edge softness, 3 amount to decolour (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let c = encode(s.rgb);
    let hc = rgb2hsv(clamp(c, vec3f(0.0), vec3f(1.0)));
    let hk = rgb2hsv(clamp(encode(params.values[0].rgb), vec3f(0.0), vec3f(1.0)));
    let dh = abs(hc.x - hk.x);
    // Greys have no hue: they count as far from any colour.
    let d = mix(1.0, min(dh, 1.0 - dh) * 2.0, smoothstep(0.02, 0.15, hc.y));
    let tol = params.values[1].x / 100.0;
    let keep = 1.0 - smoothstep(tol, tol + params.values[2].x / 100.0 + 1e-4, d);
    let o = mix(c, vec3f(luma(c)), (1.0 - keep) * params.values[3].x / 100.0);
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
