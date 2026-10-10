// 0 segments, 1 angle (degrees), 2 centre (%).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let c = pct(params.values[2].xy);
    let q = to_square(i.uv, c);
    let n = max(round(params.values[0].x), 2.0);
    let seg = TAU / n;
    let a0 = radians(params.values[1].x);
    let a = atan2(q.y, q.x) - a0;
    var t = a - seg * floor(a / seg);
    if (t > seg * 0.5) {
        t = seg - t;
    }
    let r = length(q);
    let back = vec2f(cos(t + a0), sin(t + a0)) * r;
    return src(fold(from_square(back, c)));
}
