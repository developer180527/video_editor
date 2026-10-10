// 0 horizontal, 1 vertical.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    var uv = i.uv;
    if (on(0u)) {
        uv.x = 1.0 - uv.x;
    }
    if (on(1u)) {
        uv.y = 1.0 - uv.y;
    }
    return src(uv);
}
