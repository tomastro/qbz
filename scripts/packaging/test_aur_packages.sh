#!/usr/bin/env bash
# Build and smoke all four AUR recipes in separate, clean Arch containers.
# The source packages consume the exact complete vendor archive emitted by the
# release workflow, so Cargo remains offline during both builds.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 PATH_TO_CARGO_VENDOR_ARCHIVE" >&2
    exit 2
fi

vendor_archive="$(realpath "$1")"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' crates/Cargo.toml | head -1)"
qbz_test_bin="$(realpath "${QBZ_TEST_BIN:-crates/target/release/qbz}")"
qbzd_test_bin="$(realpath "${QBZD_TEST_BIN:-crates/target/release/qbzd}")"
if [[ -n "${AUR_TEST_WORK_DIR:-}" ]]; then
    mkdir -p "$AUR_TEST_WORK_DIR"
    test_root="$(realpath "$AUR_TEST_WORK_DIR")"
    echo "Preserving Arch build workspace at $test_root"
else
    test_root="$(mktemp -d)"
    trap 'rm -rf "$test_root"' EXIT
fi

read -r -a selected_packages <<< \
    "${AUR_TEST_PACKAGES:-qbz-bin qbz qbzd-bin qbzd}"
for package in "${selected_packages[@]}"; do
    case "$package" in
        qbz-bin|qbz|qbzd-bin|qbzd) ;;
        *)
            echo "unsupported AUR test package: $package" >&2
            exit 2
            ;;
    esac
done

if [[ "${AUR_TEST_REUSE_BUILD:-0}" == 1 ]]; then
    makepkg_cleanbuild=""
else
    makepkg_cleanbuild="--cleanbuild"
fi

sources="$test_root/sources"
packages="$test_root/packages"
pacman_cache="$test_root/pacman-cache"
pacman_cache="${AUR_PACMAN_CACHE:-$pacman_cache}"
mkdir -p "$sources" "$packages" "$pacman_cache"

git archive --format=tar.gz --prefix="qbz-${version}/" \
    -o "$sources/qbz-${version}.tar.gz" HEAD
cp "$vendor_archive" "$sources/qbz-${version}-cargo-vendor.tar.xz"

qbz_dir="$test_root/qbz_${version}_amd64"
mkdir -p "$qbz_dir/icons/hicolor"/{32x32,48x48,64x64,128x128,256x256,512x512}/apps
install -m755 "$qbz_test_bin" "$qbz_dir/qbz"
install -m644 packaging/linux/qbz.desktop "$qbz_dir/qbz.desktop"
install -m644 packaging/flatpak/com.blitzfc.qbz.metainfo.xml \
    "$qbz_dir/com.blitzfc.qbz.metainfo.xml"
install -m644 LICENSE "$qbz_dir/LICENSE"
cp -r licenses "$qbz_dir/licenses"
for size in 32 48 64 128 256 512; do
    install -m644 "packaging/icons/${size}x${size}.png" \
        "$qbz_dir/icons/hicolor/${size}x${size}/apps/qbz.png"
done
tar -C "$test_root" -czf "$sources/qbz-bin-${version}-x86_64.tar.gz" \
    "qbz_${version}_amd64"

qbzd_dir="$test_root/qbzd-${version}-linux-amd64"
mkdir -p "$qbzd_dir/completions"
install -m755 "$qbzd_test_bin" "$qbzd_dir/qbzd"
install -m644 crates/qbzd/service/qbzd.service "$qbzd_dir/qbzd.service"
install -m644 packaging/linux/qbzd-standalone-README.md "$qbzd_dir/README.md"
install -m644 LICENSE "$qbzd_dir/LICENSE"
cp -r licenses "$qbzd_dir/licenses"
"$qbzd_test_bin" completions bash > "$qbzd_dir/completions/qbzd.bash"
"$qbzd_test_bin" completions zsh > "$qbzd_dir/completions/qbzd.zsh"
"$qbzd_test_bin" completions fish > "$qbzd_dir/completions/qbzd.fish"
tar -C "$test_root" -czf "$sources/qbzd-bin-${version}-x86_64.tar.gz" \
    "qbzd-${version}-linux-amd64"

for package in qbz-bin qbz qbzd-bin qbzd; do
    mkdir -p "$packages/$package"
    cp "packaging/aur/${package}/"* "$packages/$package/"
    sed -i "s/^pkgver=.*/pkgver=${version}/" "$packages/$package/PKGBUILD"
done

for package in "${selected_packages[@]}"; do
    echo "==> Clean Arch build: ${package}"
    docker run --rm \
        -e PACKAGE="$package" \
        -e VERSION="$version" \
        -e MAKEPKG_CLEANBUILD="$makepkg_cleanbuild" \
        -v "$packages/$package:/pkg" \
        -v "$sources:/sources" \
        -v "$pacman_cache:/var/cache/pacman/pkg" \
        archlinux:latest bash -euc '
            pacman -Syu --noconfirm --needed base-devel sudo
            useradd -m builder
            printf "builder ALL=(ALL) NOPASSWD: ALL\n" > /etc/sudoers.d/builder
            chown -R builder:builder /pkg
            if ! runuser -u builder -- env SRCDEST=/sources CARGO_NET_OFFLINE=true \
                MAKEPKG_CLEANBUILD="$MAKEPKG_CLEANBUILD" \
                bash -c '\''cd /pkg && makepkg -s --noconfirm --force $MAKEPKG_CLEANBUILD >makepkg.log 2>&1'\''; then
                echo "makepkg failed; last 160 log lines:" >&2
                tail -n 160 /pkg/makepkg.log >&2
                exit 1
            fi
            tail -n 20 /pkg/makepkg.log
            package_file="$(find /pkg -maxdepth 1 -type f -name "*.pkg.tar.zst" -print -quit)"
            test -n "$package_file"
            pacman -U --noconfirm "$package_file"
            case "$PACKAGE" in
                qbz|qbz-bin)
                    grep -aFq "QBZ/${VERSION}" /usr/bin/qbz
                    test -f /usr/share/licenses/${PACKAGE}/third-party/Apache-2.0-dst-decoder.txt
                    set +e
                    QT_QPA_PLATFORM=offscreen timeout 20 qbz >/tmp/qbz-smoke.log 2>&1
                    status=$?
                    set -e
                    if [[ $status -ne 124 ]]; then
                        cat /tmp/qbz-smoke.log
                        echo "qbz exited during offscreen smoke: $status" >&2
                        exit 1
                    fi
                    ;;
                qbzd|qbzd-bin)
                    qbzd version | grep -F "qbzd ${VERSION} "
                    test -f /usr/share/licenses/${PACKAGE}/third-party/Apache-2.0-dst-decoder.txt
                    qbzd completions bash >/dev/null
                    test -f /usr/lib/systemd/user/qbzd.service
                    qbzd service systemd --user builder --bin /usr/bin/qbzd \
                        >/tmp/qbzd-systemd.service
                    qbzd service openrc --user builder --bin /usr/bin/qbzd \
                        >/tmp/qbzd-openrc
                    qbzd service runit --user builder --bin /usr/bin/qbzd \
                        >/tmp/qbzd-runit
                    grep -Fq "ExecStart=/usr/bin/qbzd run" /tmp/qbzd-systemd.service
                    grep -Fq "#!/sbin/openrc-run" /tmp/qbzd-openrc
                    grep -Fq "command=\"/usr/bin/qbzd\"" /tmp/qbzd-openrc
                    grep -Fq "exec chpst -u builder:builder /usr/bin/qbzd run" /tmp/qbzd-runit
                    ;;
            esac
        '
    find "$packages/$package" -maxdepth 1 -type f -name '*.pkg.tar.zst' \
        -exec cp {} "$test_root/" \;
done

cp "$test_root"/*.pkg.tar.zst "$packages/"
echo "All AUR package builds passed."
# Preserve the products when requested by CI; the default local run cleans its
# temporary directory on exit after the proof above.
if [[ -n "${AUR_TEST_OUTPUT_DIR:-}" ]]; then
    mkdir -p "$AUR_TEST_OUTPUT_DIR"
    cp "$packages"/*.pkg.tar.zst "$AUR_TEST_OUTPUT_DIR/"
fi
