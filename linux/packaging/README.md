# Linux packaging and releases

Everything here is driven by [`.github/workflows/linux-release.yml`](../../.github/workflows/linux-release.yml).
The scripts also run on their own, so a release can be reproduced locally with Docker.

## Cutting a release

1. Bump `version` in [`linux/Cargo.toml`](../Cargo.toml) and run `cargo update -p compositor`
   in `linux/` so `Cargo.lock` matches (the build uses `--locked`).
2. Add a `<release version="X.Y.Z" date="YYYY-MM-DD"/>` line at the top of `<releases>` in
   [`io.github.calebtrueman.Compositor.metainfo.xml`](io.github.calebtrueman.Compositor.metainfo.xml).
3. Bump `pkgver` (and reset `pkgrel=1`) in [`arch/PKGBUILD`](arch/PKGBUILD) and
   [`arch-bin/PKGBUILD`](arch-bin/PKGBUILD).
4. Commit, then tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

The tag must equal `v` + the Cargo.toml version, or the workflow stops at its first job. The
workflow then:

| Job | Runs on | Produces |
| --- | --- | --- |
| `version` | ubuntu-24.04 | checks the tag against `linux/Cargo.toml` and the metainfo |
| `build (x86_64)`, `build (aarch64)` | `ubuntu:20.04` container on `ubuntu-24.04` / `ubuntu-24.04-arm` | builds and tests the binary once per architecture, then `.deb`, `.rpm`, `.tar.gz` and AppImage from it ([`scripts/package.sh`](scripts/package.sh)) |
| `smoke test` | Debian 12, Ubuntu 24.04, Fedora, openSUSE Tumbleweed containers | installs the `.deb`/`.rpm`, runs `compositor --version`, checks `ldd` and that every dlopened library got installed |
| `arch package` | `archlinux` container | `makepkg` with [`arch-bin/PKGBUILD`](arch-bin/PKGBUILD) on the x86_64 tarball, then installs it |
| `flatpak` | ubuntu-24.04 / ubuntu-24.04-arm | `flatpak-builder` with [`flatpak/io.github.calebtrueman.Compositor.yml`](flatpak/io.github.calebtrueman.Compositor.yml), bundled and test-installed |
| `publish release` | ubuntu-24.04 | `SHA256SUMS`, and the GitHub Release for the tag (uploads into it if the release already exists) |

Running the workflow by hand (**Actions → Linux release → Run workflow**) builds everything as
workflow artifacts without releasing; tick **publish** to also create a *draft* release for
`v<version>`.

### Release assets

For version `X.Y.Z`:

| File | For |
| --- | --- |
| `compositor-image-editor_X.Y.Z-1_amd64.deb`, `…_arm64.deb` | Debian, Ubuntu, Linux Mint, Pop!_OS, elementary OS, Zorin… |
| `compositor-image-editor-X.Y.Z-1.x86_64.rpm`, `….aarch64.rpm` | Fedora, RHEL / Rocky / AlmaLinux 9+, openSUSE |
| `Compositor-X.Y.Z-x86_64.AppImage`, `…-aarch64.AppImage` | any distro |
| `compositor-image-editor-bin-X.Y.Z-1-x86_64.pkg.tar.zst` | Arch Linux, EndeavourOS, Manjaro |
| `Compositor-X.Y.Z-x86_64.flatpak`, `…-aarch64.flatpak` | any distro with Flatpak |
| `compositor-image-editor-X.Y.Z-linux-x86_64.tar.gz`, `…-aarch64.tar.gz` | anything else; contains `install.sh` |
| `SHA256SUMS` | `sha256sum -c SHA256SUMS --ignore-missing` |

## Names

- **Package name: `compositor-image-editor`** (deb, rpm, Arch; `-bin` for the prebuilt Arch
  package). No distro ships a package called `compositor` today (checked Debian, Ubuntu,
  Fedora, Arch and the AUR in October 2026), but on Linux the word means a window compositor,
  and AUR/Debian already have `fht-compositor`, `kylin-wayland-compositor`,
  `steamos-compositor-plus` and the like. A bare `compositor` package would be confusing to
  search for and likely to collide later.
- **Command: `compositor`.** No package in Debian, Ubuntu, Fedora or Arch installs
  `/usr/bin/compositor` (the only file named `compositor` in Debian is a Qt example under
  `/usr/lib/*/qt5/examples/`), so the short command stays.
- **App ID: `io.github.calebtrueman.Compositor`**, used for the desktop file, AppStream
  metainfo, MIME definition, icon name, Wayland app ID / X11 WM class and Flatpak ID.

## What gets installed

All formats install the same files (the `.tar.gz` relative to its prefix):

```
/usr/bin/compositor
/usr/share/applications/io.github.calebtrueman.Compositor.desktop
/usr/share/metainfo/io.github.calebtrueman.Compositor.metainfo.xml
/usr/share/mime/packages/io.github.calebtrueman.Compositor.xml   (*.comp → application/x-compositor-project)
/usr/share/icons/hicolor/{16,32,64,128,256,512}x…/apps/io.github.calebtrueman.Compositor.png
```

The deb/rpm file lists live in `[package.metadata.deb]` and `[package.metadata.generate-rpm]` in
[`linux/Cargo.toml`](../Cargo.toml); everything else uses [`scripts/stage.sh`](scripts/stage.sh).
Keep the two in step when adding a file.

## Dependencies

The binary itself links only glibc (2.31 or newer), libm and libgcc_s. eframe (winit, glutin)
loads the rest with `dlopen` at runtime, so packaging tools cannot detect them and they are
declared by hand:

| Library | Debian/Ubuntu | Fedora | openSUSE | Arch |
| --- | --- | --- | --- | --- |
| libGL, libEGL | `libgl1`, `libegl1` | `libglvnd-glx`, `libglvnd-egl` | `libGL1`, `libEGL1` | `libglvnd` |
| xkbcommon (+ X11) | `libxkbcommon0`, `libxkbcommon-x11-0` | `libxkbcommon`, `libxkbcommon-x11` | `libxkbcommon0`, `libxkbcommon-x11-0` | `libxkbcommon`, `libxkbcommon-x11` |
| X11 | `libx11-6`, `libxcursor1`, `libxrandr2`, `libxi6` | `libX11`, `libXcursor`, `libXrandr`, `libXi` | `libX11-6`, `libXcursor1`, `libXrandr2`, `libXi6` | `libx11`, `libxcursor`, `libxrandr`, `libxi` |
| Wayland | `libwayland-client0`, `libwayland-cursor0`, `libwayland-egl1` | `libwayland-client`, `libwayland-cursor`, `libwayland-egl` | `libwayland-client0`, `libwayland-cursor0`, `libwayland-egl1` | `wayland` |

The `.rpm` requires these by soname (`libGL.so.1()(64bit)` …) so a single file resolves on
Fedora, RHEL-family and openSUSE alike. The AppImage bundles none of them: GL must come from the
host to match its driver, and the others are on every desktop.

## Building the packages locally

The release baseline is Ubuntu 20.04 (glibc 2.31). With Docker, from the repository root:

```sh
docker run --rm -it -v "$PWD:/src" -w /src ubuntu:20.04 bash -c '
  bash linux/packaging/scripts/setup-build-env.sh &&
  source ~/.cargo/env &&
  (cd linux && cargo build --release --locked) &&
  bash linux/packaging/scripts/package.sh /src/linux/dist'
```

`linux/dist/` then holds the `.deb`, `.rpm`, `.tar.gz` and AppImage for the container's
architecture (add `--platform linux/amd64` on an ARM machine to get x86_64, slowly). For the Arch
package, run `makepkg` in `arch-bin/` with the tarball beside the PKGBUILD; for the Flatpak, copy
the tarball to `flatpak/compositor-prebuilt.tar.gz` and use the commands at the top of the
manifest.

## AUR

[`arch/PKGBUILD`](arch/PKGBUILD) builds from the tagged source tarball and is meant for the AUR
as `compositor-image-editor`; [`arch-bin/PKGBUILD`](arch-bin/PKGBUILD) is the prebuilt
`compositor-image-editor-bin`. After a release is published:

```sh
cd linux/packaging/arch        # or arch-bin
updpkgsums                      # replace the SKIP checksums
makepkg --printsrcinfo > .SRCINFO
makepkg -si                     # test
```

then copy `PKGBUILD` and `.SRCINFO` into the AUR git repository and push.

## Flathub

The bundled manifest packages the prebuilt binary, which Flathub does not accept. A Flathub
submission needs a manifest that builds from source with the `org.freedesktop.Sdk.Extension.rust-stable`
extension and the crates vendored offline by
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo)
(`python3 flatpak-cargo-generator.py linux/Cargo.lock -o cargo-sources.json`), plus screenshots
in the metainfo.
