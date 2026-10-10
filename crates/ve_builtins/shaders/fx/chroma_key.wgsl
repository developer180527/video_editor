// 0 key colour, 1 tolerance, 2 softness, 3 spill suppression (%),
// 4 output (composite / alpha channel). Distances are in the chroma
// plane of the display-encoded picture.
const CB: vec3f = vec3f(-0.1146, -0.3854, 0.5);
const CR: vec3f = vec3f(0.5, -0.4542, -0.0458);

@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let c = encode(s.rgb);
    let k = encode(params.values[0].rgb);
    var cc = vec2f(dot(c, CB), dot(c, CR));
    let kc = vec2f(dot(k, CB), dot(k, CR));
    let d = distance(cc, kc) / max(length(kc), 0.05);
    let tol = params.values[1].x / 100.0;
    let a = smoothstep(tol, tol + params.values[2].x / 100.0 + 1e-4, d);
    // Spill: take the key's hue out of what remains.
    let kd = kc / max(length(kc), 1e-4);
    cc = cc - kd * max(dot(cc, kd), 0.0) * params.values[3].x / 100.0;
    let y = luma(c);
    let o = vec3f(y + 1.5748 * cc.y, y - 0.1873 * cc.x - 0.4681 * cc.y, y + 1.8556 * cc.x);
    let alpha = s.a * a;
    if (choice(4u) == 1) {
        return vec4f(vec3f(alpha), 1.0);
    }
    return premul(vec4f(decode(max(o, vec3f(0.0))), alpha));
}
