// 0 orientation (horizontal / vertical): A splits down the middle and
// its halves slide apart over B.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let h = params.progress * 0.5;
    let vertical = choice(0u) == 1;
    let x = select(i.uv.x, i.uv.y, vertical);
    let first = x + h; // the first half, moved back
    let second = x - h; // the second, moved on
    var at = -1.0;
    if (first < 0.5) {
        at = first;
    } else if (second >= 0.5) {
        at = second;
    }
    if (at < 0.0) {
        return src_b(i.uv);
    }
    return src(select(vec2f(at, i.uv.y), vec2f(i.uv.x, at), vertical));
}
