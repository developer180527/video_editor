// 0 upper left, 1 upper right, 2 lower left, 3 lower right (%), 4 feather
// (px), 5 invert: only what is inside the four points is kept.

// Signed distance to a polygon (negative inside), after Inigo Quilez.
fn sd_quad(p: vec2f, v: array<vec2f, 4>) -> f32 {
    var poly = v;
    var d = dot(p - poly[0], p - poly[0]);
    var s = 1.0;
    for (var k = 0; k < 4; k++) {
        let a = poly[k];
        let b = poly[(k + 1) % 4];
        let e = b - a;
        let w = p - a;
        let q = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
        d = min(d, dot(q, q));
        let c = vec3<bool>((p.y >= a.y), (p.y < b.y), (e.x * w.y > e.y * w.x));
        if (all(c) || !any(c)) {
            s = -s;
        }
    }
    return s * sqrt(d);
}

@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let px = uv_px(vec2f(1.0));
    let ul = pct(params.values[0].xy) * px;
    let ur = pct(params.values[1].xy) * px;
    let ll = pct(params.values[2].xy) * px;
    let lr = pct(params.values[3].xy) * px;
    let d = sd_quad(uv_px(i.uv), array<vec2f, 4>(ul, ur, lr, ll));
    let f = params.values[4].x;
    var a = select(step(d, 0.0), 1.0 - smoothstep(-0.5 * f, 0.5 * f, d), f > 0.0);
    if (on(5u)) {
        a = 1.0 - a;
    }
    return src(i.uv) * a;
}
