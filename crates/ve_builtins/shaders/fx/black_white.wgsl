// 0 reds, 1 yellows, 2 greens, 3 cyans, 4 blues, 5 magentas (%): how
// bright each colour turns; 6 tint, 7 tint colour.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let s = straight(src(i.uv));
    let v = params.values;
    let c = encode(s.rgb);
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let mid = c.r + c.g + c.b - hi - lo;
    // The largest channel picks the primary; the largest two, the secondary.
    var wp: f32;
    var ws: f32;
    if (c.r >= c.g && c.r >= c.b) {
        wp = v[0].x;
        ws = select(v[5].x, v[1].x, c.g >= c.b);
    } else if (c.g >= c.b) {
        wp = v[2].x;
        ws = select(v[3].x, v[1].x, c.r >= c.b);
    } else {
        wp = v[4].x;
        ws = select(v[5].x, v[3].x, c.g >= c.r);
    }
    let grey = lo + ((hi - mid) * wp + (mid - lo) * ws) / 100.0;
    var o = vec3f(grey);
    if (v[6].x > 0.5) {
        let t = encode(v[7].rgb);
        o = grey * t / max(luma(t), 1e-4);
    }
    return premul(vec4f(decode(max(o, vec3f(0.0))), s.a));
}
