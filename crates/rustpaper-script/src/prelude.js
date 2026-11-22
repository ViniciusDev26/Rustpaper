// WE SceneScript API, implemented in JavaScript inside the sandboxed worker.
//
// The worker evaluates this once (as a global script) before any Workshop
// script. Workshop scripts are ES modules; each one gets its own bindings
// (thisLayer, createScriptProperties, console...) from __rp.ctx(id), injected
// on the module's first line so error line numbers stay the same.
//
// Everything a script can reach is defined here: there is no host object at
// all. The host only sees what __rp.frame() returns (plain JSON).
'use strict';

// ---------------------------------------------------------------- vectors

function num(v, d) {
    const n = Number(v);
    return Number.isFinite(n) ? n : d;
}

class Vec2 {
    constructor(x, y) {
        if (x instanceof Object) { y = x.y; x = x.x; }
        else if (typeof x === 'string') { const p = x.trim().split(/\s+/); x = p[0]; y = p[1]; }
        else if (y === undefined) y = x;
        this.x = num(x, 0); this.y = num(y, 0);
    }
    static fromString(s) { return new Vec2(String(s)); }
    copy() { return new Vec2(this.x, this.y); }
    add(o) { return typeof o === 'number' ? new Vec2(this.x + o, this.y + o) : new Vec2(this.x + o.x, this.y + o.y); }
    subtract(o) { return typeof o === 'number' ? new Vec2(this.x - o, this.y - o) : new Vec2(this.x - o.x, this.y - o.y); }
    multiply(o) { return typeof o === 'number' ? new Vec2(this.x * o, this.y * o) : new Vec2(this.x * o.x, this.y * o.y); }
    divide(o) { return typeof o === 'number' ? new Vec2(this.x / o, this.y / o) : new Vec2(this.x / o.x, this.y / o.y); }
    dot(o) { return this.x * o.x + this.y * o.y; }
    length() { return Math.hypot(this.x, this.y); }
    lengthSqr() { return this.x * this.x + this.y * this.y; }
    normalize() { const l = this.length(); return l > 0 ? this.divide(l) : new Vec2(0, 0); }
    negate() { return new Vec2(-this.x, -this.y); }
    abs() { return new Vec2(Math.abs(this.x), Math.abs(this.y)); }
    min(o) { return new Vec2(Math.min(this.x, o.x), Math.min(this.y, o.y)); }
    max(o) { return new Vec2(Math.max(this.x, o.x), Math.max(this.y, o.y)); }
    round() { return new Vec2(Math.round(this.x), Math.round(this.y)); }
    floor() { return new Vec2(Math.floor(this.x), Math.floor(this.y)); }
    ceil() { return new Vec2(Math.ceil(this.x), Math.ceil(this.y)); }
    mix(o, t) { return new Vec2(this.x + (o.x - this.x) * t, this.y + (o.y - this.y) * t); }
    equals(o) { return !!o && this.x === o.x && this.y === o.y; }
    perpendicular() { return new Vec2(-this.y, this.x); }
    reflect(n) { return this.subtract(n.multiply(2 * this.dot(n))); }
    distance(o) { return this.subtract(o).length(); }
    toString() { return `${this.x} ${this.y}`; }
}

class Vec3 {
    constructor(x, y, z) {
        if (x instanceof Object) { z = x.z; y = x.y; x = x.x; }
        else if (typeof x === 'string') { const p = x.trim().split(/\s+/); x = p[0]; y = p[1]; z = p[2]; }
        else if (y === undefined) { y = x; z = x; }
        this.x = num(x, 0); this.y = num(y, 0); this.z = num(z, 0);
    }
    static fromString(s) { return new Vec3(String(s)); }
    copy() { return new Vec3(this.x, this.y, this.z); }
    add(o) { return typeof o === 'number' ? new Vec3(this.x + o, this.y + o, this.z + o) : new Vec3(this.x + o.x, this.y + o.y, this.z + num(o.z, 0)); }
    subtract(o) { return typeof o === 'number' ? new Vec3(this.x - o, this.y - o, this.z - o) : new Vec3(this.x - o.x, this.y - o.y, this.z - num(o.z, 0)); }
    multiply(o) { return typeof o === 'number' ? new Vec3(this.x * o, this.y * o, this.z * o) : new Vec3(this.x * o.x, this.y * o.y, this.z * num(o.z, 1)); }
    divide(o) { return typeof o === 'number' ? new Vec3(this.x / o, this.y / o, this.z / o) : new Vec3(this.x / o.x, this.y / o.y, this.z / num(o.z, 1)); }
    dot(o) { return this.x * o.x + this.y * o.y + this.z * o.z; }
    cross(o) { return new Vec3(this.y * o.z - this.z * o.y, this.z * o.x - this.x * o.z, this.x * o.y - this.y * o.x); }
    length() { return Math.hypot(this.x, this.y, this.z); }
    lengthSqr() { return this.dot(this); }
    normalize() { const l = this.length(); return l > 0 ? this.divide(l) : new Vec3(0, 0, 0); }
    negate() { return new Vec3(-this.x, -this.y, -this.z); }
    abs() { return new Vec3(Math.abs(this.x), Math.abs(this.y), Math.abs(this.z)); }
    min(o) { return new Vec3(Math.min(this.x, o.x), Math.min(this.y, o.y), Math.min(this.z, o.z)); }
    max(o) { return new Vec3(Math.max(this.x, o.x), Math.max(this.y, o.y), Math.max(this.z, o.z)); }
    round() { return new Vec3(Math.round(this.x), Math.round(this.y), Math.round(this.z)); }
    floor() { return new Vec3(Math.floor(this.x), Math.floor(this.y), Math.floor(this.z)); }
    ceil() { return new Vec3(Math.ceil(this.x), Math.ceil(this.y), Math.ceil(this.z)); }
    mix(o, t) { return this.add(new Vec3(o).subtract(this).multiply(t)); }
    equals(o) { return !!o && this.x === o.x && this.y === o.y && this.z === o.z; }
    reflect(n) { return this.subtract(n.multiply(2 * this.dot(n))); }
    distance(o) { return this.subtract(o).length(); }
    toString() { return `${this.x} ${this.y} ${this.z}`; }
}

class Vec4 {
    constructor(x, y, z, w) {
        if (x instanceof Object) { w = x.w; z = x.z; y = x.y; x = x.x; }
        else if (typeof x === 'string') { const p = x.trim().split(/\s+/); x = p[0]; y = p[1]; z = p[2]; w = p[3]; }
        else if (y === undefined) { y = x; z = x; w = x; }
        this.x = num(x, 0); this.y = num(y, 0); this.z = num(z, 0); this.w = num(w, 0);
    }
    static fromString(s) { return new Vec4(String(s)); }
    copy() { return new Vec4(this.x, this.y, this.z, this.w); }
    add(o) { return typeof o === 'number' ? new Vec4(this.x + o, this.y + o, this.z + o, this.w + o) : new Vec4(this.x + o.x, this.y + o.y, this.z + o.z, this.w + o.w); }
    subtract(o) { return typeof o === 'number' ? new Vec4(this.x - o, this.y - o, this.z - o, this.w - o) : new Vec4(this.x - o.x, this.y - o.y, this.z - o.z, this.w - o.w); }
    multiply(o) { return typeof o === 'number' ? new Vec4(this.x * o, this.y * o, this.z * o, this.w * o) : new Vec4(this.x * o.x, this.y * o.y, this.z * o.z, this.w * o.w); }
    divide(o) { return typeof o === 'number' ? new Vec4(this.x / o, this.y / o, this.z / o, this.w / o) : new Vec4(this.x / o.x, this.y / o.y, this.z / o.z, this.w / o.w); }
    dot(o) { return this.x * o.x + this.y * o.y + this.z * o.z + this.w * o.w; }
    length() { return Math.sqrt(this.dot(this)); }
    normalize() { const l = this.length(); return l > 0 ? this.divide(l) : new Vec4(0, 0, 0, 0); }
    mix(o, t) { return this.add(o.subtract(this).multiply(t)); }
    equals(o) { return !!o && this.x === o.x && this.y === o.y && this.z === o.z && this.w === o.w; }
    toString() { return `${this.x} ${this.y} ${this.z} ${this.w}`; }
}

// ---------------------------------------------------------------- built-in modules

const WEMath = {
    PI: Math.PI,
    deg2rad: Math.PI / 180,
    rad2deg: 180 / Math.PI,
    mix(a, b, t) {
        if (a instanceof Object && typeof a.mix === 'function') return a.mix(b, t);
        return a + (b - a) * t;
    },
    lerp(a, b, t) { return WEMath.mix(a, b, t); },
    clamp(v, lo, hi) { return Math.min(Math.max(v, lo), hi); },
    smoothStep(e0, e1, x) {
        const t = Math.min(Math.max((x - e0) / (e1 - e0), 0), 1);
        return t * t * (3 - 2 * t);
    },
    smoothstep(e0, e1, x) { return WEMath.smoothStep(e0, e1, x); },
    fract(x) { return x - Math.floor(x); },
    sign(x) { return Math.sign(x); },
    step(e, x) { return x < e ? 0 : 1; },
};

const WEColor = {
    rgb2hsv(c) {
        const r = c.x, g = c.y, b = c.z;
        const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
        let h = 0;
        if (d > 0) {
            if (max === r) h = ((g - b) / d) % 6;
            else if (max === g) h = (b - r) / d + 2;
            else h = (r - g) / d + 4;
            h /= 6;
            if (h < 0) h += 1;
        }
        return new Vec3(h, max > 0 ? d / max : 0, max);
    },
    hsv2rgb(c) {
        const h = ((c.x % 1) + 1) % 1, s = c.y, v = c.z;
        const i = Math.floor(h * 6), f = h * 6 - i;
        const p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s);
        const rgb = [[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]][i % 6];
        return new Vec3(rgb[0], rgb[1], rgb[2]);
    },
    normalizeColor(c) { return new Vec3(c).divide(255); },
    expandColor(c) { return new Vec3(c).multiply(255); },
};

const WEVector = {
    angleVector2(deg) {
        const r = deg * Math.PI / 180;
        return new Vec2(Math.cos(r), Math.sin(r));
    },
    vectorAngle2(v) { return Math.atan2(v.y, v.x) * 180 / Math.PI; },
};

// ---------------------------------------------------------------- runtime

const __rp = (() => {
    const MAX_LOGS = 64;
    const layers = [];          // states, in scene order
    const byId = new Map();
    const scripts = [];         // { def, ns, props, failed: Set, last }
    const logs = [];
    const sent = new Map();     // layer id -> JSON of each field last sent
    const timers = [];
    const audioBuffers = [];
    let clockMs = 0;
    let nowMs = 0;
    let warnedCreate = false;
    let running = -1;
    const MAX_CREATED = 4096;
    const created = [];         // already reported to the host
    const pendingCreated = [];
    const camera = { zoom: 1, sent: 1 };  // thisScene camera transforms
    let nextId = 0;             // set above every scene id in init()
    function currentLayer() {
        const s = scripts.find((x) => x.def.id === running);
        return s ? s.def.layer : -1;
    }

    function log(msg) {
        if (logs.length < MAX_LOGS) logs.push(String(msg).slice(0, 500));
    }
    function fmt(args) {
        return args.map((a) => {
            if (typeof a === 'string') return a;
            try { return JSON.stringify(a); } catch (e) { return String(a); }
        }).join(' ');
    }
    function makeConsole(id) {
        const out = (lvl) => (...a) => log(`[script ${id}] ${lvl}${fmt(a)}`);
        return { log: out(''), info: out(''), debug: out(''), warn: out('warn: '), error: out('error: ') };
    }

    // property value <-> JS
    function toJs(kind, v) {
        switch (kind) {
            case 'vec2': return new Vec2(Array.isArray(v) ? { x: v[0], y: v[1] } : v);
            case 'vec3': return new Vec3(Array.isArray(v) ? { x: v[0], y: v[1], z: v[2] } : v);
            case 'vec4': return new Vec4(Array.isArray(v) ? { x: v[0], y: v[1], z: v[2], w: v[3] } : v);
            case 'bool': return !!v;
            case 'string': return v == null ? '' : String(v);
            default: return num(v, 0);
        }
    }
    function fromJs(kind, v) {
        switch (kind) {
            case 'vec2': { const o = new Vec2(v); return [o.x, o.y]; }
            case 'vec3': { const o = typeof v === 'number' ? new Vec3(v) : new Vec3(v); return [o.x, o.y, o.z]; }
            case 'vec4': { const o = new Vec4(v); return [o.x, o.y, o.z, o.w]; }
            case 'bool': return !!v;
            case 'string': return v == null ? '' : String(v);
            default: return num(v, 0);
        }
    }

    // ------------------------------------------------------------ layers
    const RAD2DEG = 180 / Math.PI;
    const LAYER_FIELDS = { origin: 'vec3', scale: 'vec3', angles: 'vec3', color: 'vec3',
                           alpha: 'float', brightness: 'float', visible: 'bool', text: 'string',
                           playing: 'bool', anim_time: 'float' };

    function stub(extra) {
        const o = { play() {}, pause() {}, stop() {}, isPlaying() { return false; },
                    setFrame() {}, getFrame() { return 0; }, frameCount: 1, fps: 0, rate: 1,
                    duration: 0, ...extra };
        return o;
    }

    // Texture animation (sprite sheet / GIF flipbook) of an image layer.
    // `st.frames` holds its frame durations (from the .tex); layers without
    // one get the stub. The clock only runs here once a script asked for the
    // animation: it starts at engine.runtime (the phase the renderer was
    // drawing from the scene clock) and is reported as `anim_time`, which
    // then drives the layer's flipbook instead of the scene clock.
    // WE semantics: stop() rewinds to the first frame and holds it, pause()
    // holds the current frame, play() resumes; the animation loops.
    // Chainsaw Man 3577990983's five fireworks stop themselves on their last
    // frame and replay after a random delay; frame 0 of the sheet is empty,
    // so a stopped firework is invisible (WE's capture shows the sky empty
    // between bursts; we played all five non-stop).
    const textureAnims = [];
    function textureAnimation(st) {
        const frames = Array.isArray(st.frames) ? st.frames.map((d) => Math.max(num(d, 0), 0)) : [];
        const total = frames.reduce((a, b) => a + b, 0);
        if (frames.length < 2 || !(total > 0)) return stub();
        if (st.anim) return st.anim.api;
        const a = { t: engine.runtime % total, playing: true, rate: 1 };
        const frameAt = (t) => {
            let k = 0;
            for (; k < frames.length - 1; k++) {
                if (t < frames[k]) break;
                t -= frames[k];
            }
            return k;
        };
        const startOf = (n) => frames.slice(0, n).reduce((s, d) => s + d, 0);
        a.advance = (dt) => {
            if (a.playing) a.t = (a.t + dt * a.rate) % total;
            if (a.t < 0) a.t += total;
            st.anim_time = a.t;
        };
        a.api = {
            frameCount: frames.length,
            duration: total,
            fps: frames.length / total,
            get rate() { return a.rate; },
            set rate(v) { a.rate = num(v, a.rate); },
            play() { a.playing = true; },
            pause() { a.playing = false; },
            stop() { a.playing = false; a.t = 0; st.anim_time = 0; },
            isPlaying() { return a.playing; },
            getFrame() { return frameAt(a.t); },
            setFrame(n) {
                const k = Math.max(0, Math.min(Math.floor(num(n, 0)), frames.length - 1));
                a.t = startOf(k);
                st.anim_time = a.t;
            },
        };
        st.anim = a;
        st.anim_time = a.t;
        textureAnims.push(a);
        return a.api;
    }

    class Layer {
        constructor(st) { Object.defineProperty(this, '_s', { value: st }); }
        get name() { return this._s.name; }
        get id() { return this._s.id; }
        get origin() { return toJs('vec3', this._s.origin); }
        set origin(v) { this._s.origin = fromJs('vec3', v); }
        get originalOrigin() { return toJs('vec3', this._s.origin0); }
        get scale() { return toJs('vec3', this._s.scale); }
        set scale(v) { this._s.scale = fromJs('vec3', v); }
        // scripts see angles in DEGREES (scene.json stores radians)
        get angles() { return toJs('vec3', this._s.angles.map((a) => a * RAD2DEG)); }
        set angles(v) { this._s.angles = fromJs('vec3', v).map((a) => a / RAD2DEG); }
        get size() { return toJs('vec2', this._s.size); }
        set size(v) { this._s.size = fromJs('vec2', v); }
        get color() { return toJs('vec3', this._s.color); }
        set color(v) { this._s.color = fromJs('vec3', v); }
        get alpha() { return this._s.alpha; }
        set alpha(v) { this._s.alpha = num(v, this._s.alpha); }
        get brightness() { return this._s.brightness; }
        set brightness(v) { this._s.brightness = num(v, this._s.brightness); }
        get visible() { return this._s.visible; }
        set visible(v) { this._s.visible = !!v; }
        get text() { return this._s.text == null ? '' : this._s.text; }
        set text(v) { this._s.text = v == null ? '' : String(v); }
        isVisible() { return this._s.visible; }
        setVisible(v) { this._s.visible = !!v; }
        getParent() { const p = byId.get(this._s.parent); return p ? p.layer : null; }
        getChildren() { return layers.filter((s) => s.parent === this._s.id).map((s) => s.layer); }
        getAnimation() { return stub(); }
        getTextureAnimation() { return textureAnimation(this._s); }
        getEffect() { return { visible: true, getMaterial() { return {}; } }; }
        getEffectCount() { return 0; }
        getAnimationLayer() { return stub(); }
        // sound layers (playback stays with the engine for now)
        play() { this._s.playing = true; }
        stop() { this._s.playing = false; }
        pause() { this._s.playing = false; }
        isPlaying() { return this._s.playing === true; }
        getAnimationLayerCount() { return 0; }
    }

    function addLayer(st) {
        st.origin0 = st.origin.slice();
        if (st.playing === undefined) st.playing = null; // a diff baseline
        st.anim_time = null; // set once a script uses getTextureAnimation()
        st.layer = new Layer(st);
        layers.push(st);
        byId.set(st.id, st);
    }

    const scene = {
        getLayer(key) {
            const st = typeof key === 'number' ? byId.get(key)
                : layers.find((s) => s.name === key);
            return st ? st.layer : null;
        },
        getLayerCount() { return layers.length; },
        getLayerIndex(l) { return l ? layers.findIndex((s) => s.layer === l) : -1; },
        enumerateLayers() { return layers.map((s) => s.layer); },
        // layer clones (visualizer bars...): the host loads them and draws
        // them where sortLayer() put them
        createLayer(cfg) {
            if (created.length + pendingCreated.length >= MAX_CREATED) {
                if (!warnedCreate) { warnedCreate = true; log(`createLayer: more than ${MAX_CREATED} layers`); }
                return null;
            }
            const c = typeof cfg === 'string' ? { image: cfg } : Object.assign({}, cfg || {});
            const vec = (v, d) => (v == null ? d : fromJs('vec3', v));
            const st = {
                id: nextId++, name: String(c.name || ''), kind: c.text != null ? 'text' : 'image', parent: null,
                origin: vec(c.origin, [0, 0, 0]), scale: vec(c.scale, [1, 1, 1]), angles: vec(c.angles, [0, 0, 0]),
                size: c.size != null ? fromJs('vec2', c.size) : [0, 0], color: vec(c.color, [1, 1, 1]),
                alpha: num(c.alpha, 1), brightness: num(c.brightness, 1),
                visible: c.visible == null ? true : !!c.visible,
                text: c.text != null ? String(c.text) : null,
            };
            addLayer(st);
            st.fresh = true;
            const plain = {};
            for (const [k, v] of Object.entries(c)) {
                plain[k] = v instanceof Object && typeof v.toString === 'function' && 'x' in v ? v.toString() : v;
            }
            pendingCreated.push({ id: st.id, script: running, template: currentLayer(), config: plain, before: null });
            return st.layer;
        },
        destroyLayer(l) { if (l && l._s) l._s.visible = false; },
        sortLayer(l, index) {
            if (!l || !l._s) return;
            const from = layers.indexOf(l._s);
            if (from < 0) return;
            layers.splice(from, 1);
            const to = Math.max(0, Math.min(num(index, layers.length), layers.length));
            layers.splice(to, 0, l._s);
            const pc = pendingCreated.find((c) => c.id === l._s.id);
            if (pc) pc.before = to + 1 < layers.length ? layers[to + 1].id : null;
        },
        // only the zoom reaches the renderer (Persona 3 3151551777 sets it
        // from a user property); eye/center/up are read-only placeholders
        getCameraTransforms() {
            return { eye: new Vec3(0, 0, 0), center: new Vec3(0, 0, -1), up: new Vec3(0, 1, 0), zoom: camera.zoom };
        },
        setCameraTransforms(t) {
            if (t && typeof t.zoom === 'number' && isFinite(t.zoom)) camera.zoom = t.zoom;
        },
        getInitialLayerConfig() { return null; },
    };

    // ------------------------------------------------------------ globals
    const engine = {
        frametime: 0,
        runtime: 0,
        timeOfDay: 0,
        canvasSize: new Vec2(0, 0),
        screenResolution: new Vec2(0, 0),
        userProperties: {},
        AUDIO_RESOLUTION_16: 16,
        AUDIO_RESOLUTION_32: 32,
        AUDIO_RESOLUTION_64: 64,
        registerAudioBuffers(res) {
            const n = [16, 32, 64].includes(res) ? res : 16;
            const b = { resolution: n, left: new Float32Array(n), right: new Float32Array(n),
                        average: new Float32Array(n) };
            audioBuffers.push(b);
            return b;
        },
        registerAsset(path) { return { path: String(path) }; },
        setTimeout(fn, ms) { return addTimer(fn, ms, false); },
        setInterval(fn, ms) { return addTimer(fn, ms, true); },
        clearTimeout(id) { clearTimer(id); },
        clearInterval(id) { clearTimer(id); },
        openUserShortcut() {},
        isDesktopDevice() { return true; },
        isMobileDevice() { return false; },
        isWallpaper() { return true; },
        isScreensaver() { return false; },
        isRunningInEditor() { return false; },
        isPortrait() { return engine.screenResolution.y > engine.screenResolution.x; },
        isLandscape() { return !engine.isPortrait(); },
    };
    const input = {
        cursorWorldPosition: new Vec3(0, 0, 0),
        cursorScreenPosition: new Vec2(0, 0),
        cursorLeftDown: false,
    };
    const store = new Map();
    const localStorage = {
        get(k) { return store.has(k) ? store.get(k) : undefined; },
        set(k, v) { store.set(k, v); },
        remove(k) { store.delete(k); },
        delete(k) { store.delete(k); },
        clear() { store.clear(); },
    };

    let timerId = 1;
    function addTimer(fn, ms, repeat) {
        if (typeof fn !== 'function') return 0;
        const t = { id: timerId++, fn, at: nowMs + Math.max(num(ms, 0), 0), every: Math.max(num(ms, 0), 1), repeat };
        timers.push(t);
        return t.id;
    }
    function clearTimer(id) {
        const k = timers.findIndex((t) => t.id === id);
        if (k >= 0) timers.splice(k, 1);
    }
    function runTimers() {
        for (let guard = 0; guard < 1000; guard++) {
            let k = -1;
            for (let i = 0; i < timers.length; i++) {
                if (timers[i].at <= nowMs && (k < 0 || timers[i].at < timers[k].at)) k = i;
            }
            if (k < 0) return;
            const t = timers[k];
            if (t.repeat) t.at += t.every; else timers.splice(k, 1);
            try { t.fn(); } catch (e) { log(`timer error: ${e}`); }
        }
    }

    // Date: the host's clock (deterministic offline renders), local time
    // through the TZ the host passes in the environment
    const RealDate = Date;
    class HostDate extends RealDate {
        constructor(...a) { if (a.length === 0) super(clockMs); else super(...a); }
        static now() { return clockMs; }
    }
    globalThis.Date = HostDate;

    // ------------------------------------------------------------ script properties
    function scriptPropertiesBuilder(stored) {
        const decl = {};
        const b = {
            finish() {
                const out = {};
                for (const [name, d] of Object.entries(decl)) {
                    let v = Object.prototype.hasOwnProperty.call(stored, name) ? stored[name] : d.value;
                    if (v instanceof Object && 'value' in v && !(v instanceof Vec3)) v = v.value;
                    if (d.type === 'color' || d.value instanceof Vec3) v = new Vec3(v);
                    else if (typeof d.value === 'number') v = num(v, d.value);
                    else if (typeof d.value === 'boolean') v = typeof v === 'string' ? v === 'true' || v === '1' : !!v;
                    out[name] = v;
                }
                return out;
            },
        };
        const p = new Proxy(b, {
            get(t, key) {
                if (key in t) return t[key];
                if (typeof key === 'string' && key.startsWith('add')) {
                    const type = key.slice(3).toLowerCase();
                    return (o) => { if (o && o.name != null) decl[o.name] = { type, value: o.value }; return p; };
                }
                return undefined;
            },
        });
        return p;
    }

    // ------------------------------------------------------------ host entry points
    function call(s, fn, ...args) {
        if (s.failed.has(fn) || typeof s.ns[fn] !== 'function') return undefined;
        running = s.def.id;
        try {
            return s.ns[fn](...args);
        } catch (e) {
            s.failed.add(fn);
            log(`[script ${s.def.id} ${s.def.property}] ${fn}: ${e}${e && e.stack ? '\n' + e.stack : ''}`);
            return undefined;
        }
    }

    function current(s) {
        if (s.def.binding === 'layer') {
            const st = byId.get(s.def.layer);
            if (st && s.def.property === 'angles') return st.layer.angles;
            if (st && s.def.property in LAYER_FIELDS) return toJs(s.def.kind, st[s.def.property]);
        }
        return toJs(s.def.kind, s.value);
    }
    function store_(s, v) {
        if (v === undefined) return;
        const out = fromJs(s.def.kind, v);
        if (s.def.binding === 'layer') {
            const st = byId.get(s.def.layer);
            if (st && s.def.property === 'angles') { st.layer.angles = v; return; }
            if (st && s.def.property in LAYER_FIELDS) { st[s.def.property] = out; return; }
        }
        s.value = out;
    }

    function collect() {
        const ups = [];
        let zoom;
        if (camera.zoom !== camera.sent) { camera.sent = camera.zoom; zoom = camera.zoom; }
        for (const st of layers) {
            let prev = sent.get(st.id);
            if (!prev) { prev = {}; sent.set(st.id, prev); }
            let up = null;
            const fresh = st.fresh;
            st.fresh = false;
            for (const f of Object.keys(LAYER_FIELDS)) {
                const j = JSON.stringify(st[f]);
                if (fresh && st[f] != null) { prev[f] = j; (up || (up = { id: st.id }))[f] = st[f]; continue; }
                if (prev[f] === undefined) { prev[f] = j; continue; }
                if (prev[f] !== j) { prev[f] = j; (up || (up = { id: st.id }))[f] = st[f]; }
            }
            if (up) ups.push(up);
        }
        const values = [];
        for (const s of scripts) {
            if (s.def.binding !== 'value') continue;
            const j = JSON.stringify(s.value);
            if (j !== s.sent) { s.sent = j; values.push([s.def.id, s.value]); }
        }
        const fresh = pendingCreated.splice(0);
        created.push(...fresh);
        const out = { created: fresh, layers: ups, values, zoom, logs: logs.splice(0) };
        return JSON.stringify(out);
    }

    // cursorMove for every script when the cursor moves; cursorEnter /
    // cursorLeave for the scripts whose layer the cursor enters or leaves
    let lastCursor = null;
    let hovered = new Set();
    function cursorEvents(f) {
        const c = f.cursor || [0, 0];
        const ev = () => ({ worldPosition: new Vec3(c[0], c[1], 0), screenPosition: new Vec2(c[0], c[1]) });
        const moved = !lastCursor || lastCursor[0] !== c[0] || lastCursor[1] !== c[1];
        lastCursor = c;
        const now = new Set(f.hover || []);
        for (const s of scripts) {
            if (moved) call(s, 'cursorMove', ev());
            const was = hovered.has(s.def.layer), is = now.has(s.def.layer);
            if (is && !was) call(s, 'cursorEnter', ev());
            if (was && !is) call(s, 'cursorLeave', ev());
        }
        hovered = now;
    }

    function applyFrame(f) {
        engine.frametime = num(f.dt, 0);
        engine.runtime = num(f.time, 0);
        for (const a of textureAnims) a.advance(engine.frametime);
        engine.timeOfDay = num(f.daytime, 0);
        clockMs = num(f.clock_ms, 0);
        nowMs = engine.runtime * 1000;
        const c = f.cursor || [0, 0];
        input.cursorWorldPosition = new Vec3(c[0], c[1], 0);
        input.cursorScreenPosition = new Vec2(c[0], c[1]);
        const L = f.audio_left || [], R = f.audio_right || [];
        for (const b of audioBuffers) {
            const step = 64 / b.resolution;
            for (let i = 0; i < b.resolution; i++) {
                let l = 0, r = 0;
                for (let k = 0; k < step; k++) { l = Math.max(l, num(L[i * step + k], 0)); r = Math.max(r, num(R[i * step + k], 0)); }
                b.left[i] = l; b.right[i] = r; b.average[i] = (l + r) / 2;
            }
        }
    }

    return {
        // per-module bindings (see the module prefix in the worker)
        ctx(id) {
            const s = scripts.find((x) => x.def.id === id);
            const st = s && byId.get(s.def.layer);
            const layer = st ? st.layer : null;
            return {
                thisLayer: layer,
                thisObject: layer,
                thisScene: scene,
                console: makeConsole(id),
                createScriptProperties: () => scriptPropertiesBuilder(s ? s.def.script_properties || {} : {}),
            };
        },
        init(json) {
            const init = JSON.parse(json);
            for (const st of init.layers) addLayer(st);
            nextId = Math.max(0, ...init.layers.map((l) => l.id)) + 1000000;
            engine.canvasSize = new Vec2(init.canvas[0], init.canvas[1]);
            camera.zoom = camera.sent = num(init.zoom, 1);
            engine.screenResolution = new Vec2(init.screen[0], init.screen[1]);
            engine.userProperties = init.user_properties || {};
            for (const def of init.scripts) {
                scripts.push({ def, ns: {}, failed: new Set(), value: def.value, sent: JSON.stringify(def.value) });
            }
            // the first collect() only records the starting state (and
            // returns nothing but logs)
            return collect();
        },
        // the worker evaluated a module: its namespace (exports)
        attach(id, ns) {
            const s = scripts.find((x) => x.def.id === id);
            if (s) s.ns = ns;
        },
        moduleFailed(id, err) {
            log(`[script ${id}] load: ${err}`);
        },
        start(json) {
            applyFrame(JSON.parse(json));
            for (const s of scripts) store_(s, call(s, 'init', current(s)));
            for (const s of scripts) call(s, 'applyUserProperties', engine.userProperties);
            for (const s of scripts) store_(s, call(s, 'update', current(s)));
            return collect();
        },
        frame(json) {
            const f = JSON.parse(json);
            applyFrame(f);
            cursorEvents(f);
            runTimers();
            for (const s of scripts) store_(s, call(s, 'update', current(s)));
            return collect();
        },
        // a script ran past the time budget: never call it again
        disable(id) {
            const s = scripts.find((x) => x.def.id === id);
            if (s) { s.ns = {}; log(`[script ${id}] disabled: exceeded the time budget`); }
        },
        running() { return running; },
        // pending logs and layer writes (after module loading, after an
        // aborted frame)
        frameLogs() { return collect(); },
        log,
        engine, input, localStorage,
        shared: {},
    };
})();

globalThis.engine = __rp.engine;
globalThis.input = __rp.input;
globalThis.localStorage = __rp.localStorage;
globalThis.shared = __rp.shared;
globalThis.Vec2 = Vec2;
globalThis.Vec3 = Vec3;
globalThis.Vec4 = Vec4;
globalThis.WEMath = WEMath;
globalThis.WEColor = WEColor;
globalThis.WEVector = WEVector;
globalThis.setTimeout = (f, ms) => __rp.engine.setTimeout(f, ms);
globalThis.setInterval = (f, ms) => __rp.engine.setInterval(f, ms);
globalThis.clearTimeout = (id) => __rp.engine.clearTimeout(id);
globalThis.clearInterval = (id) => __rp.engine.clearInterval(id);
globalThis.__rp = __rp;
