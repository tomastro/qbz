# QBZ AppImage

## About AppImage

AppImage is a portable format that bundles the application and its dependencies.
However, it still uses system libraries and commands from the host system.

## Dependencies

### Required (Hard Dependencies)

The following must be installed on the host system for QBZ to work:

- **ALSA library** (`libasound2` on Debian/Ubuntu, `alsa-lib` on Arch)
- **GTK3** and **WebKit2GTK**
- **OpenSSL**

These are typically already installed on most Linux distributions.

### Optional (Recommended)

For the best experience, install these optional packages:

#### alsa-utils

**Package names:**
- Debian/Ubuntu: `alsa-utils`
- Fedora/RHEL: `alsa-utils`
- Arch: `alsa-utils`

**Why it's needed:**
- Provides the `aplay` command used by QBZ to enumerate ALSA devices
- Without it, you'll see raw device names like `front:CARD=CS201,DEV=0`
- With it, you'll see friendly names like "Cambridge Audio USB Front Output"

**What happens without it:**
- ALSA Direct backend falls back to CPAL enumeration (still works)
- Device names won't be as descriptive
- All functionality works, just with less friendly UI

#### PipeWire/PulseAudio

**For PipeWire:**
- `pipewire-alsa` - ALSA compatibility
- `pipewire-pulse` - PulseAudio compatibility

**For PulseAudio:**
- `pulseaudio`
- `pulseaudio-alsa`

**Why it's needed:**
- Required for PipeWire/PulseAudio audio backends
- Required for DAC Passthrough feature (uses `pw-metadata`)

## Building AppImage

TODO: Add build instructions using appimagetool or similar

## Installation

1. Download the AppImage from releases
2. Make it executable: `chmod +x QBZ-*.AppImage`
3. Run: `./QBZ-*.AppImage`

## System Integration

To integrate with your desktop environment:

```bash
# Install optional tool for desktop integration
sudo apt install appimaged  # Debian/Ubuntu
# or
yay -S appimagelauncher     # Arch

# AppImage will automatically appear in your application menu
```

## Troubleshooting

### "No ALSA devices found"

Install `alsa-utils`:
```bash
sudo apt install alsa-utils  # Debian/Ubuntu
sudo dnf install alsa-utils  # Fedora
sudo pacman -S alsa-utils    # Arch
```

### "DAC Passthrough not working"

Ensure PipeWire is running:
```bash
systemctl --user status pipewire
```

Install pw-metadata:
```bash
sudo apt install pipewire-bin  # Debian/Ubuntu (includes pw-metadata)
```
