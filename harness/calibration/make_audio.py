#!/usr/bin/env python3
"""make_audio.py <out dir>

Builds a loose WE scene that shows WE's audio spectrum as gray cells, to
calibrate Rustpaper's analyzer (rustpaper_gl::audio) against WE:

  top third     g_AudioSpectrum64Left,  64 cells, left = band 0 (lowest)
  middle third  g_AudioSpectrum64Right, 64 cells
  bottom third  g_AudioSpectrum16Left,  16 cells
  gray = band value (0 black .. 1 white)

Play a known track while capturing it (run_audio.sh), then read the cells
back with measure_audio.py. The shader is plain WE dialect, no SceneScript.
"""
import json, os, struct, sys

out = sys.argv[1]
W, H = 1920, 1080

VERT = """
uniform mat4 g_ModelViewProjectionMatrix;
attribute vec3 a_Position;
attribute vec2 a_TexCoord;
varying vec2 v_TexCoord;
void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord;
}
"""

FRAG = """
uniform sampler2D g_Texture0;
uniform float g_AudioSpectrum64Left[64];
uniform float g_AudioSpectrum64Right[64];
uniform float g_AudioSpectrum16Left[16];
varying vec2 v_TexCoord;
void main() {
	float v = 0.0;
	float row = floor(v_TexCoord.y * 3.0);
	if (row < 0.5) {
		int b = int(min(floor(v_TexCoord.x * 64.0), 63.0));
		for (int i = 0; i < 64; ++i) { if (i == b) v = g_AudioSpectrum64Left[i]; }
	} else if (row < 1.5) {
		int b = int(min(floor(v_TexCoord.x * 64.0), 63.0));
		for (int i = 0; i < 64; ++i) { if (i == b) v = g_AudioSpectrum64Right[i]; }
	} else {
		int b = int(min(floor(v_TexCoord.x * 16.0), 15.0));
		for (int i = 0; i < 16; ++i) { if (i == b) v = g_AudioSpectrum16Left[i]; }
	}
	// keep the texture referenced (some compilers drop unused samplers)
	v += texSample2D(g_Texture0, v_TexCoord).r * 0.0;
	// 0.25 + 0.75 v: a gray floor shows the shader runs even in silence
	v = 0.25 + 0.75 * v;
	gl_FragColor = vec4(v, v, v, 1.0);
}
"""


def tex_rgba(w, h, px):
    """TEXV0005 / TEXI0001 / TEXB0002, format 0 (RGBA8888), 1 mip, uncompressed."""
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1)
    d += struct.pack("<I", 1)
    d += struct.pack("<5I", w, h, 0, len(px), len(px))
    return d + px


for sub in ("materials", "models", "shaders"):
    os.makedirs(os.path.join(out, sub), exist_ok=True)
open(os.path.join(out, "shaders", "audiobars.vert"), "w").write(VERT)
open(os.path.join(out, "shaders", "audiobars.frag"), "w").write(FRAG)
open(os.path.join(out, "materials", "white.tex"), "wb").write(tex_rgba(4, 4, bytes([255] * 64)))
json.dump({"passes": [{"shader": "audiobars", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["white"]}]},
          open(os.path.join(out, "materials", "bars.json"), "w"), indent=1)
json.dump({"material": "materials/bars.json"}, open(os.path.join(out, "models", "bars.json"), "w"))
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "bars", "image": "models/bars.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
# WE only feeds g_AudioSpectrum* to wallpapers that declare audio support.
# `general` must also carry `properties`: without it WE shows a black window.
json.dump({"type": "scene", "file": "scene.json", "title": "audio spectrum calibration",
           "general": {"properties": {}, "supportsaudioprocessing": True}},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
