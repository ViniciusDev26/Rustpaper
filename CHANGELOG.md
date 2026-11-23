# Changelog

## [0.11.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.10.0...v0.11.0) (2026-11-23)


### Features

* **engine:** let web wallpapers take clicks ([#55](https://github.com/ViniciusDev26/Rustpaper/issues/55)) ([557f93c](https://github.com/ViniciusDev26/Rustpaper/commit/557f93c0b09c9f559a7f9d728167e3631c098d18))

## [0.10.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.9.0...v0.10.0) (2026-11-23)


### Features

* **selector:** choose where a wallpaper goes before applying it ([#53](https://github.com/ViniciusDev26/Rustpaper/issues/53)) ([ebaae6a](https://github.com/ViniciusDev26/Rustpaper/commit/ebaae6a3c1168fb3ddf22d9c5394618bc987c170))

## [0.9.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.8.0...v0.9.0) (2026-11-23)


### Features

* **lock:** forward the pointer over the lock screen's wallpaper ([#51](https://github.com/ViniciusDev26/Rustpaper/issues/51)) ([38befca](https://github.com/ViniciusDev26/Rustpaper/commit/38befca2e50fa17e338a4f7cbbf04385a85105ac))

## [0.8.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.7.1...v0.8.0) (2026-11-23)


### Features

* **engine:** forward the user's pointer to wallpaper engine's windows ([#49](https://github.com/ViniciusDev26/Rustpaper/issues/49)) ([60f76f2](https://github.com/ViniciusDev26/Rustpaper/commit/60f76f2d920be1810fdc97790dbe7fd0b4529b7d))

## [0.7.1](https://github.com/ViniciusDev26/Rustpaper/compare/v0.7.0...v0.7.1) (2026-11-23)


### Bug Fixes

* **engine:** wallpaper engine's volume, mute and pause that work ([#47](https://github.com/ViniciusDev26/Rustpaper/issues/47)) ([39551bc](https://github.com/ViniciusDev26/Rustpaper/commit/39551bc40dc7a0a86597e82a7334858f31d476c4))
* stage socket cleanup, wine driver switch notice ([#46](https://github.com/ViniciusDev26/Rustpaper/issues/46)) ([82254a6](https://github.com/ViniciusDev26/Rustpaper/commit/82254a6ee09a395bde852e9b5ab2331fc384d13b))

## [0.7.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.6.0...v0.7.0) (2026-11-23)


### Features

* **engine:** opt-in wine wayland driver for wallpaper engine ([#45](https://github.com/ViniciusDev26/Rustpaper/issues/45)) ([2484953](https://github.com/ViniciusDev26/Rustpaper/commit/24849531524076de4c97f821169e88bb93f101a4))
* **engine:** play video wallpapers in mpv with hardware decoding ([#44](https://github.com/ViniciusDev26/Rustpaper/issues/44)) ([a05f378](https://github.com/ViniciusDev26/Rustpaper/commit/a05f3782441685ef9347478752a45c11764cc023))
* **engine:** render a wallpaper once for every monitor showing it ([#41](https://github.com/ViniciusDev26/Rustpaper/issues/41)) ([61ce067](https://github.com/ViniciusDev26/Rustpaper/commit/61ce0673ec8dbbb7a6d5ea061fbccb608159541d))


### Bug Fixes

* **engine:** respect the desktop's stay-awake switch ([#43](https://github.com/ViniciusDev26/Rustpaper/issues/43)) ([d8fb860](https://github.com/ViniciusDev26/Rustpaper/commit/d8fb860f20d01095f1c27de9d4cd64c0e78279db))

## [0.6.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.5.0...v0.6.0) (2026-11-23)


### Features

* **engine:** key repeat in the lock screen's password field ([3f3e11c](https://github.com/ViniciusDev26/Rustpaper/commit/3f3e11ceb560788ee4b44bbd623e1eb015baf791))


### Bug Fixes

* **engine:** stop Wallpaper Engine within systemd's stop timeout ([d9087e5](https://github.com/ViniciusDev26/Rustpaper/commit/d9087e581fff85c10861c7c0df933fd4cafb8fb5))
* **packaging:** no debug split package for rustpaper-bin ([318b9ac](https://github.com/ViniciusDev26/Rustpaper/commit/318b9ac6be7defd55b5e9de50a7f57fc472a1d9e))

## [0.5.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.4.0...v0.5.0) (2026-11-23)


### Features

* **core:** wallpapers by role, per monitor ([4d98c1d](https://github.com/ViniciusDev26/Rustpaper/commit/4d98c1d5b7c4e79b7f6af865080ce6aca9ac9b59))
* **engine:** animated lock screen with lock adapters ([d9569da](https://github.com/ViniciusDev26/Rustpaper/commit/d9569daa8b57d3560a4137a5a16b27c904a4dcc8))
* **engine:** per-monitor wallpapers, screensaver and lock wallpaper ([c3505ef](https://github.com/ViniciusDev26/Rustpaper/commit/c3505effcc577ceca4e818a15a9c644f2e80cb13))
* **selector:** animated lock screen switch ([384bc01](https://github.com/ViniciusDev26/Rustpaper/commit/384bc01d28b57f09530b7451f55bad39581dd8c1))
* **selector:** use a wallpaper for a monitor, the lock screen or the screensaver ([3bcca26](https://github.com/ViniciusDev26/Rustpaper/commit/3bcca26fb1e447f23d79083f434e3baec2881fc6))

## [0.4.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.3.0...v0.4.0) (2026-11-22)


### Features

* **engine:** private stage, Rustpaper's own compositor for WE's windows ([59d6417](https://github.com/ViniciusDev26/Rustpaper/commit/59d641750a9db01291f0e78ee32c4ff0a7b7f1b7))


### Bug Fixes

* **engine:** stop WE's whole launch on restart ([34d095e](https://github.com/ViniciusDev26/Rustpaper/commit/34d095effc64c25abd86b0512a730fcaf005fcc4))
* **engine:** sway adapter on nested sessions, stage repairs without WE ([45f418a](https://github.com/ViniciusDev26/Rustpaper/commit/45f418a5393e97f695f95ea19a78eb5ca7a608b9))
* **packaging:** depend on xorg-xwayland instead of hyprland ([02d5db0](https://github.com/ViniciusDev26/Rustpaper/commit/02d5db0566134e7793ff581c336815e78cdc27e8))

## [0.3.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.2.0...v0.3.0) (2026-11-22)


### ⚠ BREAKING CHANGES

* **selector:** "Start on login" is now a systemd user service; the old ~/.config/autostart/rustpaper.desktop is removed.
* **engine:** Wallpaper Engine must be installed through Steam with Proton and Hyprland 0.56+ is required. rustpaper-render and rustpaper-script-worker are gone.

### Features

* **core:** shared selector state and WE quality settings as data ([7e4cda0](https://github.com/ViniciusDev26/Rustpaper/commit/7e4cda02c5039fdf3abc6da5e011fd4efd231573))
* **engine:** render wallpapers with the real Wallpaper Engine under Proton ([f4bc12a](https://github.com/ViniciusDev26/Rustpaper/commit/f4bc12a17732809556a5390266a426b82fb68011))
* **engine:** service mode, in-place reload and playback rules ([475775d](https://github.com/ViniciusDev26/Rustpaper/commit/475775daa229e50ebae1ab9e70c1710f7e9a12fc))
* **script:** drive flipbooks from getTextureAnimation() ([d932668](https://github.com/ViniciusDev26/Rustpaper/commit/d932668631fdda230813189f14c5ee00c994e5ec))
* **selector:** engine as a systemd service, Performance tab, every wallpaper ([31b7801](https://github.com/ViniciusDev26/Rustpaper/commit/31b7801bbc42bd13c17da843ed26ef8fcda48808))
* **selector:** match Hyprland and follow the Omarchy theme ([fd775dd](https://github.com/ViniciusDev26/Rustpaper/commit/fd775dd367ba6469cf78f9fb454e1306826334b6))
* **selector:** match Hyprland/Omarchy and theme the desktop from the wallpaper ([6af8526](https://github.com/ViniciusDev26/Rustpaper/commit/6af8526c0f575345ab2a275928d8defc75a49d2c))
* **selector:** theme the desktop from the wallpaper's colors ([6b96b63](https://github.com/ViniciusDev26/Rustpaper/commit/6b96b632d053a6ca1d3066e5840419143ca8d45b))


### Bug Fixes

* **ci:** drop the glslangValidator check from the CI image ([851edf3](https://github.com/ViniciusDev26/Rustpaper/commit/851edf384e187fb8722b65a1f054dea07578089d))
* **gl:** camera parallax moves objects away from the cursor ([91ab676](https://github.com/ViniciusDev26/Rustpaper/commit/91ab6766ba31c10401935f0196b194a830c4f678))
* **gl:** camera parallax moves objects away from the cursor ([73469fe](https://github.com/ViniciusDev26/Rustpaper/commit/73469fe70eb30841bc08847f550a0c4737b72187))
* **gl:** Chainsaw Man Reze fireworks, parallax and dock backdrop ([7f0566e](https://github.com/ViniciusDev26/Rustpaper/commit/7f0566ecd00d010345e8703fead995f8e876b68a))
* **gl:** children take their ancestor's parallax depth ([214454b](https://github.com/ViniciusDev26/Rustpaper/commit/214454b37b208a5e80f719d20c8d4930602bad09))
* **gl:** honor image layer alignment ([5039525](https://github.com/ViniciusDev26/Rustpaper/commit/503952545c174792bc6de7f8ca5c55af5bee38bf))
* **gl:** image children inherit their ancestor's parallax depth ([826defe](https://github.com/ViniciusDev26/Rustpaper/commit/826defe563678931b05c9fd369b1ca35ffdd6c6e))
* **gl:** inherited parallax shifts children from the ancestor's origin ([3c03091](https://github.com/ViniciusDev26/Rustpaper/commit/3c03091a5b7236825ecf917b00139118c2c93498))
* **gl:** inherited parallax shifts children from the ancestor's origin ([959af41](https://github.com/ViniciusDev26/Rustpaper/commit/959af41a19b9d835fcdee6502c63deccbc278a47))
* **gl:** keep text texels outside the glyphs transparent black ([6029018](https://github.com/ViniciusDev26/Rustpaper/commit/6029018da0fdf43a2c83b1c0f3bbc3f922a24dc0))
* **gl:** layer alignment and script camera zoom (Persona 3 passes, Reze 0.889) ([15efd7e](https://github.com/ViniciusDev26/Rustpaper/commit/15efd7e32f876c032aff1f302435e1458ba8aa58))
* **gl:** Lucy puppet effects, child parallax and text texels ([b30b3e6](https://github.com/ViniciusDev26/Rustpaper/commit/b30b3e660318d33435d471d765919ee5ae658c5a))
* **gl:** parallax strength is amount, influence scales the cursor reach ([21c583a](https://github.com/ViniciusDev26/Rustpaper/commit/21c583a9e16988e0c777b194bf577158c6f26e2f))
* **gl:** parallax strength is amount, influence scales the cursor reach ([35a7cb8](https://github.com/ViniciusDev26/Rustpaper/commit/35a7cb8397cbcb6ca2084a3c105d57cd07aa1acb))
* **gl:** run puppet effects on the texture before the warp ([d27374a](https://github.com/ViniciusDev26/Rustpaper/commit/d27374a668cddcd4761df21d2cee5ec3cb113af7))
* **gl:** set g_LayerModelMatrix in every pass ([92a6e0b](https://github.com/ViniciusDev26/Rustpaper/commit/92a6e0bb99b976f638f386cc2139c0b07ddd05a0))
* **gl:** the script camera zoom multiplies general.zoom ([6884362](https://github.com/ViniciusDev26/Rustpaper/commit/68843628d16550780c507ec00028254da78614e6))

## [0.2.0](https://github.com/ViniciusDev26/Rustpaper/compare/v0.1.0...v0.2.0) (2026-11-22)


### Features

* **gl:** render shape quad objects ([505b8ff](https://github.com/ViniciusDev26/Rustpaper/commit/505b8ff2cbeb36308ad09a9c373d167dfb3e99a7))
* **gl:** render shape quad objects ([c119213](https://github.com/ViniciusDev26/Rustpaper/commit/c1192131908b71b6c145d4f3c862bd11cd6b2686))
* **selector:** master volume slider ([4115721](https://github.com/ViniciusDev26/Rustpaper/commit/4115721a472e5426fbe82fe30197b7e10846ec12))
* **selector:** rewrite the UI in React, Tailwind, Zustand and Zod ([2fd37a9](https://github.com/ViniciusDev26/Rustpaper/commit/2fd37a96b1c15f6726b2938b292e9a35f2ee561c))
* **selector:** rewrite the UI in React, Tailwind, Zustand and Zod ([bd2cb92](https://github.com/ViniciusDev26/Rustpaper/commit/bd2cb92755b4919fb210a368b7b7a03d7be5c5ac))
* **selector:** sort the library by subscription, unsubscribe from it ([261630e](https://github.com/ViniciusDev26/Rustpaper/commit/261630e015969ddb7c263ecc3cda0776550e2b82))
* **selector:** store tab for the Steam Workshop ([5f5813e](https://github.com/ViniciusDev26/Rustpaper/commit/5f5813e8a34f57c7fb965dc996efb1096f3f4fb2))
* Steam Workshop store in the selector ([0de279e](https://github.com/ViniciusDev26/Rustpaper/commit/0de279ec0f6d632b439ff9ef1b72f9e91f8c422c))
* **workshop:** bridge to the Steam Workshop for the selector ([51e8463](https://github.com/ViniciusDev26/Rustpaper/commit/51e846347593cf79e62461391b413f7399f330f3))
* **workshop:** subscription times command ([0d6982e](https://github.com/ViniciusDev26/Rustpaper/commit/0d6982ef5bb91e794c4547a8ef7304113c37d994))


### Bug Fixes

* **engine:** no compositor fade when switching wallpapers ([31d9f7d](https://github.com/ViniciusDev26/Rustpaper/commit/31d9f7db521f22947f4044f55fd663abdbe7bc3a))
* **engine:** stop scene sounds with the engine ([b7df516](https://github.com/ViniciusDev26/Rustpaper/commit/b7df516e099d80aa6ee1153b3356cbccc30ca280))
* **gl:** additive puppet layers are relative to the bind pose ([de2ee3a](https://github.com/ViniciusDev26/Rustpaper/commit/de2ee3a638667b78f4548ecfaa09442443a333d2))
* **gl:** additive puppet layers are relative to the bind pose ([a1a414a](https://github.com/ViniciusDev26/Rustpaper/commit/a1a414a8afbe8a1c09abaa41a1e354f905a337d7))
* **gl:** stop leaking scene targets every frame ([8e45e28](https://github.com/ViniciusDev26/Rustpaper/commit/8e45e28357f854f1a402fd01c68fca6b404c5d2a))
* **selector:** only replace engines that are not its children ([d0c316d](https://github.com/ViniciusDev26/Rustpaper/commit/d0c316d3878f1ce10c9a7348dddb8ab5d7bba72e))
* **selector:** replace engines it did not start ([383d076](https://github.com/ViniciusDev26/Rustpaper/commit/383d0760b6bf17ae4c8cedae77aeb53ddaae7a9c))
* sound and engine leftovers, switch flash, master volume ([aa6738e](https://github.com/ViniciusDev26/Rustpaper/commit/aa6738e77ef4120d0cc1e729af620fa117236f1f))
* **workshop:** find libsteam_api.so in both cargo build layouts ([90855cf](https://github.com/ViniciusDev26/Rustpaper/commit/90855cf14b23c7c758262394895c5b3923b8a3c7))
* **workshop:** find libsteam_api.so in both cargo build layouts ([ae0cb07](https://github.com/ViniciusDev26/Rustpaper/commit/ae0cb07df8bf14d0ce595fc24dfc10903cb2e5c5))

## 0.1.0 (2026-11-21)


### Features

* **packaging:** release tarball and AUR PKGBUILDs ([6f444ef](https://github.com/ViniciusDev26/Rustpaper/commit/6f444efe258da9faf03cfd6dd57a000dc48e1caf))
