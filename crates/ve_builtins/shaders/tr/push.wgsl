// 0 direction (from left / right / top / bottom): B pushes A out.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let d = travel(0u);
    let ub = i.uv + d * (1.0 - p);
    if (inside(ub)) {
        return src_b(ub);
    }
    return src_clear(i.uv - d * p);
}
