// Shared by every shader in the standard pack: prepended to each effect's
// source, after the host's prelude (sdk/WGSL_CONTRACT.md).

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;
// Rec.709 luma weights. ABI v1 shaders are not told the working space;
// in ACEScg these are close, not exact.
const LUMA: vec3f = vec3f(0.2126, 0.7152, 0.0722);

fn luma(c: vec3f) -> f32 {
    return dot(c, LUMA);
}

// Premultiplied → straight colour (zero where transparent), and back.
fn straight(p: vec4f) -> vec4f {
    if (p.a <= 1e-6) {
        return vec4f(0.0);
    }
    return vec4f(p.rgb / p.a, p.a);
}

fn premul(c: vec4f) -> vec4f {
    return vec4f(c.rgb * c.a, c.a);
}

// The monitor's encoding (a 2.4 power), mirrored for negatives. Tone
// controls work on these values, so 50 % means 50 % on screen.
fn encode(c: vec3f) -> vec3f {
    return sign(c) * pow(abs(c), vec3f(1.0 / 2.4));
}

fn decode(c: vec3f) -> vec3f {
    return sign(c) * pow(abs(c), vec3f(2.4));
}

// Sampling. Level 0 is the picture; lower levels are 2×2 averages.
fn tap(t: texture_2d<f32>, uv: vec2f, lod: f32, clear: bool) -> vec4f {
    let c = textureSampleLevel(t, source_sampler, uv, lod);
    if (clear && !inside(uv)) {
        return vec4f(0.0);
    }
    return c;
}

fn src(uv: vec2f) -> vec4f {
    return textureSampleLevel(source, source_sampler, uv, 0.0);
}

fn src_b(uv: vec2f) -> vec4f {
    return textureSampleLevel(source_b, source_sampler, uv, 0.0);
}

fn inside(uv: vec2f) -> bool {
    return all(uv >= vec2f(0.0)) && all(uv <= vec2f(1.0));
}

// Transparent outside the picture, rather than its edge pixels repeated.
fn src_clear(uv: vec2f) -> vec4f {
    return tap(source, uv, 0.0, true);
}

// Folded back into the picture at its edges (a mirror-repeat).
fn fold(uv: vec2f) -> vec2f {
    return vec2f(1.0) - abs(vec2f(1.0) - 2.0 * fract(uv * 0.5));
}

// The source's size in texels, and its width over height.
fn texels() -> vec2f {
    return vec2f(textureDimensions(source));
}

fn aspect() -> f32 {
    let s = texels();
    return s.x / max(s.y, 1.0);
}

// Picture pixels (sizes as the user types them) → uv along x and y.
fn px_uv(n: f32) -> vec2f {
    return vec2f(n * params.scale) / texels();
}

// uv → picture pixels.
fn uv_px(uv: vec2f) -> vec2f {
    return uv * texels() / params.scale;
}

// A point given in percent of the frame (0..100), as uv.
fn pct(v: vec2f) -> vec2f {
    return v * 0.01;
}

// Around `c`, in picture heights on both axes (circles stay round), and back.
fn to_square(uv: vec2f, c: vec2f) -> vec2f {
    return vec2f((uv.x - c.x) * aspect(), uv.y - c.y);
}

fn from_square(q: vec2f, c: vec2f) -> vec2f {
    return vec2f(q.x / aspect() + c.x, q.y + c.y);
}

fn rot(v: vec2f, a: f32) -> vec2f {
    let c = cos(a);
    let s = sin(a);
    return vec2f(c * v.x - s * v.y, s * v.x + c * v.y);
}

// A direction in degrees, clockwise from up (y grows downwards).
fn heading(deg: f32) -> vec2f {
    let a = radians(deg);
    return vec2f(sin(a), -cos(a));
}

// Choice parameter `n` as an integer.
fn choice(n: u32) -> i32 {
    return i32(params.values[n].x + 0.5);
}

fn on(n: u32) -> bool {
    return params.values[n].x > 0.5;
}

// Hashes and value noise: the same for the same position.
fn hash12(p: vec2f) -> f32 {
    var q = fract(vec3f(p.x, p.y, p.x) * 0.1031);
    q = q + vec3f(dot(q, q.yzx + 33.33));
    return fract((q.x + q.y) * q.z);
}

fn vnoise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2f(1.0, 0.0));
    let c = hash12(i + vec2f(0.0, 1.0));
    let d = hash12(i + vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2f, octaves: i32) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    var total = 0.0;
    for (var o = 0; o < octaves; o++) {
        v += a * vnoise(q);
        total += a;
        q = q * 2.03 + vec2f(17.0, 9.0);
        a *= 0.5;
    }
    return v / max(total, 1e-6);
}

// Hue, saturation, value (all 0..1) of an encoded colour, and back.
fn rgb2hsv(c: vec3f) -> vec3f {
    let k = vec4f(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4f(c.bg, k.wz), vec4f(c.gb, k.xy), step(c.b, c.g));
    let q = mix(vec4f(p.xyw, c.r), vec4f(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3f(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsv2rgb(c: vec3f) -> vec3f {
    let k = vec4f(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, vec3f(0.0), vec3f(1.0)), c.y);
}

// A Gaussian blur of `t` at `uv`, sigma in texels: a 9×9 grid read from
// the mip level whose texels are about sigma/2 wide, so any radius is
// smooth at a fixed cost. `clear`: transparent beyond the edges.
fn blur2(t: texture_2d<f32>, uv: vec2f, sigma: f32, clear: bool) -> vec4f {
    if (sigma < 0.3) {
        return tap(t, uv, 0.0, clear);
    }
    let lod = max(log2(sigma / 2.0), 0.0);
    let d = vec2f(sigma * 0.75) / vec2f(textureDimensions(t));
    var acc = vec4f(0.0);
    var wsum = 0.0;
    for (var y = -4; y <= 4; y++) {
        for (var x = -4; x <= 4; x++) {
            let k = vec2f(f32(x), f32(y));
            let w = exp(-0.28125 * dot(k, k)); // 0.75² / 2
            acc += w * tap(t, uv + k * d, lod, clear);
            wsum += w;
        }
    }
    return acc / wsum;
}

// A Gaussian blur along `dir` (a unit vector), sigma in texels.
fn blur1(t: texture_2d<f32>, uv: vec2f, dir: vec2f, sigma: f32, clear: bool) -> vec4f {
    if (sigma < 0.3) {
        return tap(t, uv, 0.0, clear);
    }
    let n = i32(clamp(ceil(3.0 * sigma), 1.0, 32.0));
    let spacing = 3.0 * sigma / f32(n);
    let lod = clamp(log2(spacing / 1.5), 0.0, 2.0);
    let d = dir * spacing / vec2f(textureDimensions(t));
    var acc = vec4f(0.0);
    var wsum = 0.0;
    for (var k = -n; k <= n; k++) {
        let x = f32(k) * spacing / sigma;
        let w = exp(-0.5 * x * x);
        acc += w * tap(t, uv + f32(k) * d, lod, clear);
        wsum += w;
    }
    return acc / wsum;
}

// The average of `t` along the segment from uv - d/2 to uv + d/2 (d in
// uv): a motion blur. About a tap per 1.5 texels, at most 64.
fn smear(t: texture_2d<f32>, uv: vec2f, d: vec2f, clear: bool) -> vec4f {
    let len = length(d * vec2f(textureDimensions(t)));
    if (len < 1.0) {
        return tap(t, uv, 0.0, clear);
    }
    let n = i32(clamp(ceil(len / 1.5), 2.0, 64.0));
    let lod = max(log2(len / f32(n) / 1.5), 0.0);
    var acc = vec4f(0.0);
    for (var k = 0; k < n; k++) {
        let f = f32(k) / f32(n - 1) - 0.5;
        acc += tap(t, uv + d * f, lod, clear);
    }
    return acc / f32(n);
}

// Transitions: how far B has come in at a pixel whose turn is `f` (0..1:
// 0 first, 1 last), at progress `p`, with an edge `soft` wide (0..1).
// Exactly 0 at p = 0 and exactly 1 at p = 1, whatever `f` and `soft`.
fn reveal(f: f32, p: f32, soft: f32) -> f32 {
    let s = max(soft, 1e-4);
    return smoothstep(0.0, s, p * (1.0 + s) - clamp(f, 0.0, 1.0));
}

// A transition's direction choice: From Left, From Right, From Top, From
// Bottom, as the way the incoming picture moves.
fn travel(n: u32) -> vec2f {
    switch choice(n) {
        case 1: { return vec2f(-1.0, 0.0); }
        case 2: { return vec2f(0.0, 1.0); }
        case 3: { return vec2f(0.0, -1.0); }
        default: { return vec2f(1.0, 0.0); }
    }
}
