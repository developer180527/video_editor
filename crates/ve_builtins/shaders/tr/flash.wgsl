// 0 intensity (stops at the peak): a burst of light hides the cut.
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let p = params.progress;
    let o = mix(src(i.uv), src_b(i.uv), smoothstep(0.4, 0.6, p));
    let bump = 4.0 * p * (1.0 - p);
    return vec4f(o.rgb * exp2(params.values[0].x * bump * bump), o.a);
}
