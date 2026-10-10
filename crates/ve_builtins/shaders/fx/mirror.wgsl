// 0 reflection centre (%), 1 reflection angle (degrees): one side of the
// line shows the other, reflected.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[0].xy);
    let a = radians(params.values[1].x);
    let n = vec2f(cos(a), sin(a));
    var q = to_square(i.uv, c);
    let d = dot(q, n);
    if (d > 0.0) {
        q = q - 2.0 * d * n;
    }
    return src_clear(from_square(q, c));
}
