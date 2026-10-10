// 0 from, 1 to, 2 change (hue / + lightness / + saturation), 3 tolerance,
// 4 softness.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let c = encode(s.rgb);
    let hc = rgb2hsv(clamp(c, vec3f(0.0), vec3f(1.0)));
    let hf = rgb2hsv(clamp(encode(params.values[0].rgb), vec3f(0.0), vec3f(1.0)));
    let ht = rgb2hsv(clamp(encode(params.values[1].rgb), vec3f(0.0), vec3f(1.0)));
    let dh = abs(hc.x - hf.x);
    let d = mix(1.0, min(dh, 1.0 - dh) * 2.0, smoothstep(0.02, 0.15, hc.y));
    let tol = params.values[3].x / 100.0;
    let amt = 1.0 - smoothstep(tol, tol + params.values[4].x / 100.0 + 1e-4, d);
    if (amt <= 0.0) {
        return premul(s);
    }
    var h = hc;
    h.x = fract(hc.x + (ht.x - hf.x) * amt);
    let mode = choice(2u);
    if (mode >= 1) {
        h.z = hc.z * mix(1.0, ht.z / max(hf.z, 1e-4), amt);
    }
    if (mode >= 2) {
        h.y = clamp(hc.y * mix(1.0, ht.y / max(hf.y, 1e-4), amt), 0.0, 1.0);
    }
    return premul(vec4f(decode(max(hsv2rgb(h), vec3f(0.0))), s.a));
}
