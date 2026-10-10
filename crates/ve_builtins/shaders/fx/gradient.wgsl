// A generator. 0 start (%), 1 start colour, 2 end (%), 3 end colour,
// 4 shape (linear / radial). Blended as the monitor shows it.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    // Output pixels per uv: a generator has no source to measure.
    let size = vec2f(1.0 / max(abs(dpdx(i.uv.x)), 1e-9), 1.0 / max(abs(dpdy(i.uv.y)), 1e-9));
    let k = vec2f(size.x / size.y, 1.0);
    let a = pct(params.values[0].xy) * k;
    let b = pct(params.values[2].xy) * k;
    let p = i.uv * k;
    let ab = b - a;
    var t: f32;
    if (choice(4u) == 1) {
        t = length(p - a) / max(length(ab), 1e-6);
    } else {
        t = dot(p - a, ab) / max(dot(ab, ab), 1e-12);
    }
    t = clamp(t, 0.0, 1.0);
    let c0 = params.values[1];
    let c1 = params.values[3];
    let c = decode(mix(encode(c0.rgb), encode(c1.rgb), t));
    return premul(vec4f(c, mix(c0.a, c1.a, t)));
}
