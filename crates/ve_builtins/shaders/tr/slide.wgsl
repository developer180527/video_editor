// 0 direction (from left / right / top / bottom): B slides in over A.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let ub = i.uv + travel(0u) * (1.0 - params.progress);
    if (inside(ub)) {
        return src_b(ub);
    }
    return src(i.uv);
}
