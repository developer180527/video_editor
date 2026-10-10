// 0 axis (vertical / horizontal), 1 perspective (%): A turns over like a
// card, and B is on its back.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let th = PI * p;
    let horizontal = choice(0u) == 1;
    // In picture heights: x across the turning axis, y along it.
    let sq = vec2f((i.uv.x - 0.5) * aspect(), i.uv.y - 0.5);
    let xy = select(sq, sq.yx, horizontal);
    let dist = mix(6.0, 1.2, params.values[1].x / 100.0) * max(aspect(), 1.0);
    // A ray from the eye at (0, 0, -dist) through the screen point, met
    // with the card turned by th about the axis.
    let den = dist * cos(th) - xy.x * sin(th);
    if (abs(den) < 1e-6) {
        return vec4f(0.0);
    }
    let t = dist * cos(th) / den;
    if (t <= 0.0) {
        return vec4f(0.0);
    }
    let hit = vec3f(xy * t, dist * (t - 1.0));
    let back = p > 0.5;
    // Where on the card: B, on the back, reads the right way round.
    var u = dot(hit, vec3f(cos(th), 0.0, sin(th)));
    if (back) {
        u = -u;
    }
    let local = select(vec2f(u, hit.y), vec2f(hit.y, u), horizontal);
    let card = vec2f(local.x / aspect() + 0.5, local.y + 0.5);
    if (!inside(card)) {
        return vec4f(0.0);
    }
    let shade = 1.0 - 0.35 * abs(sin(th));
    let c = select(src(card), src_b(card), back);
    return vec4f(c.rgb * shade, c.a);
}
