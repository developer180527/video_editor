// 0 block size (px), 1 sharp colours (the block's centre, not its average).
@fragment
fn effect(i: EffectIn) -> @location(0) vec4f {
    let b = max(params.values[0].x, 1.0);
    let cell = floor(uv_px(i.uv) / b);
    let uv = (cell + 0.5) * b * params.scale / texels();
    let lod = select(log2(max(b * params.scale, 1.0)), 0.0, on(1u));
    return tap(source, uv, lod, false);
}
