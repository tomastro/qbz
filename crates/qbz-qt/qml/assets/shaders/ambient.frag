#version 440

// Ambient — the app-wide dynamic background, a 1:1 GLSL port of
// crates/qbz-ui/ui/shaders/ambient.wgsl (WGSL, wgpu scene 7). Every constant,
// every orbit and every curve below is the same number the reference uses; the
// only differences are mechanical (WGSL -> GLSL 440, uniform block instead of a
// bind group, and `time` arrives as a QML property instead of a shared frame
// uniform).
//
// WHY THIS IS A SHADER AND NOT A CANVAS. The Canvas version this replaces
// painted four radial gradients over a BLACK base, so anywhere outside a blob's
// radius the window was #000000 — measured on the owner's 2026-08-04 side by
// side: Qt at (1600,600) and (1690,880) was (0,0,0) while Slint was (58,51,5)
// and (29,27,10). The reference cannot produce black: `col` is the
// metaball-WEIGHTED album colour (the field divides out, so it is defined
// everywhere) and the darkest it is ever scaled is `mix(0.42, 1.12, shape)` at
// shape = 0. The floor is 42% of the album colour across the WHOLE window,
// which is why the Slint reads as one wash that brightens into lobes and the
// Canvas read as spotlights on black. That floor is not reachable by adding
// gradients — it needs the per-pixel weighted average, i.e. this.
//
// Canvas remains as the fallback arm for the software scene graph (see
// AmbientField.qml), where it now paints the same floor as a flat base.
//
// The uniform block is the ShaderEffect contract: qt_Matrix and qt_Opacity are
// supplied by Qt, everything after them must exist as a `property real` /
// `property color` on the effect item. Qt resolves them BY NAME out of the
// .qsb's reflection data, so the QML declaration order is free — but a name
// with no matching property is silently left at zero, which is why every
// member below is spelled identically on both sides.

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    vec4 primary;
    vec4 secondary;
    vec4 accent;
    float qt_Opacity;
    float time;
    float levelSmooth;
    float resX;
    float resY;
    float dim;
};

// QSB-SKIP-GLES100 — build.rs drops the `100 es` variant for this file. GLSL ES
// 1.00 has no uint, SPIRV-Cross re-types the hash constants below as int, the
// large ones go negative and qsb fails the WHOLE bake. Dropping the ES 2.0
// level costs nothing here: AmbientField falls back to its Canvas arm wherever
// the effect cannot load, and the hash cannot become float-based (see below).
//
// INTEGER lattice hash (bit-exact), wgsl:hash2. The reference's comment is
// load-bearing and applies verbatim here: float hashes leave the result to f32
// rounding, so the SAME lattice corner hashed from two adjacent cells can
// disagree and draw a "wallpaper join" seam through the warp field. uint
// arithmetic has no rounding.
float hash2(vec2 p) {
    uint h = uint(int(p.x)) * 0x8da6b343u + uint(int(p.y)) * 0xd8163841u;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return float(h >> 8u) * (1.0 / 16777216.0);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    float a = hash2(i);
    float b = hash2(i + vec2(1.0, 0.0));
    float c = hash2(i + vec2(0.0, 1.0));
    float d = hash2(i + vec2(1.0, 1.0));
    vec2 w = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

float fbm(vec2 p) {
    float v = 0.0;
    float amp = 0.55;
    vec2 q = p;
    v += amp * vnoise(q); q *= 2.02; amp *= 0.5;
    v += amp * vnoise(q); q *= 2.03; amp *= 0.5;
    v += amp * vnoise(q);
    return v;
}

// Metaball potential (r^2/d^2): summed over several movers the iso-surface
// merges and splits organically. Unlike a gaussian blob this has a LONG TAIL —
// that tail is what fills the window between the lobes, and it is precisely
// what a canvas radial gradient (which terminates at its radius) cannot do.
float mball(vec2 uv, vec2 c, float r) {
    vec2 d = uv - c;
    return (r * r) / (dot(d, d) + 0.0009);
}

void main() {
    // Aspect-correct so the drift looks even on wide windows. NOTE the space
    // this puts us in: x spans [0, aspect] and y spans [0, 1], so the radius
    // below is a fraction of the window HEIGHT — the Canvas port had it as
    // 0.36 * max(W, H), which on a 1700x1400 window made the blobs 29% too big.
    float aspect = resX / max(resY, 1.0);
    vec2 uv = qt_TexCoord0;
    uv.x *= aspect;

    // Clock. Fast enough that the flow is visible in seconds (blob orbits are
    // ~10-18 s); `levelSmooth` adds a gentle breathe when audio is flowing.
    float t = time * 0.75;
    float breathe = 1.0 + 0.12 * levelSmooth;

    // Two-octave domain warp. The low octave does the big organic flow; the
    // high octave stays small and lower-frequency — a strong high-freq warp
    // folds the metaball field into sharp creases that read as hard edges over
    // the translucent UI.
    vec2 w1 = vec2(
        fbm(uv * 1.3 + vec2(t * 0.5, t * 0.2)),
        fbm(uv * 1.3 + vec2(-t * 0.3, t * 0.45))
    );
    vec2 w2 = vec2(
        fbm(uv * 2.3 + vec2(-t * 0.6, t * 0.45)),
        fbm(uv * 2.3 + vec2(t * 0.55, -t * 0.35))
    );
    vec2 p = uv + (w1 - 0.5) * 0.68 + (w2 - 0.5) * 0.14;

    // Four album-coloured metaballs on big wandering orbits.
    vec3 c4 = mix(primary.rgb, accent.rgb, 0.5);
    vec2 cA = vec2((0.32 + 0.30 * sin(t * 0.40)) * aspect,       0.42 + 0.30 * cos(t * 0.33));
    vec2 cB = vec2((0.66 + 0.30 * sin(t * 0.35 + 2.1)) * aspect, 0.56 + 0.32 * cos(t * 0.29 + 1.3));
    vec2 cC = vec2((0.50 + 0.34 * cos(t * 0.31 + 4.0)) * aspect, 0.36 + 0.30 * sin(t * 0.45 + 3.2));
    vec2 cD = vec2((0.46 + 0.32 * sin(t * 0.27 + 5.3)) * aspect, 0.64 + 0.28 * cos(t * 0.49 + 0.7));

    float rr = 0.34 * breathe;
    float fA = mball(p, cA, rr);
    float fB = mball(p, cB, rr * 0.95);
    float fC = mball(p, cC, rr * 0.88);
    float fD = mball(p, cD, rr * 0.82);
    float field = fA + fB + fC + fD;

    // Metaball-weighted album colour (which lobe dominates here).
    vec3 col = (primary.rgb * fA + secondary.rgb * fB + accent.rgb * fC + c4 * fD)
        / (field + 0.0001);

    // The amoeba structure: the iso-surface. A WIDE smoothstep so the lobes melt
    // into the base over a long soft gradient instead of a hard rim, and a
    // gentler bright/dark spread so transitions never snap. 0.42 is the floor
    // this whole file exists for.
    float shape = smoothstep(0.45, 3.4, field);
    col *= mix(0.42, 1.12, shape);

    // Push saturation/contrast a little so the lobes read as album colour, not
    // a muddy average — but not so hard it re-sharpens the transitions.
    float luma = dot(col, vec3(0.299, 0.587, 0.114));
    col = mix(vec3(luma), col, 1.28);

    // Vertical falloff -> a touch darker at the very top/bottom edges so chrome
    // (titlebar, player bar) sits on calmer colour. QUADRATIC, not the linear
    // black ramp the Canvas used.
    float vshade = 1.0 - 0.16 * pow(abs(qt_TexCoord0.y - 0.5) * 2.0, 2.0);
    col *= vshade;

    // Overall brightness — vivid; the dim scrim above this layer provides the
    // legibility dim, so the base can stay bright without glaring.
    col = clamp(col, vec3(0.0), vec3(1.0)) * 0.92;

    // A touch of grain (dither) to break up 8-bit banding on the smooth
    // gradient — banding rings read as faint hard edges too.
    float grain = (vnoise(qt_TexCoord0 * vec2(resX, resY) * 0.5) - 0.5) * 0.022;
    col += vec3(grain);

    // The legibility scrim, FOLDED IN. The reference paints it as a separate
    // full-window black Rectangle at `dim` alpha over the field
    // (AppShell.slint:236-241), which is arithmetically `field * (1 - dim)` —
    // identical to doing it here, one multiply instead of a whole extra
    // full-screen blend pass in every frame the scene redraws. With the
    // background up the shell is already an all-alpha stack, so a pass removed
    // is a pass removed from EVERY frame, not just the 30 the field owns.
    col *= (1.0 - clamp(dim, 0.0, 1.0));

    col = clamp(col, vec3(0.0), vec3(1.0));
    // Premultiplied, like every other effect in this tree.
    fragColor = vec4(col * qt_Opacity, qt_Opacity);
}
