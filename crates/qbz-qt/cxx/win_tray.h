// Windows notification-area icon. C ABI so the Rust seam (src/tray_windows.rs)
// can drive it exactly as tray_linux.rs drives the StatusNotifierItem and
// tray_macos.rs drives the NSStatusItem.
//
// EVERY function here is GUI-THREAD ONLY. The icon owns a real (hidden) window
// and a message loop; touching it from tokio would post to a thread that never
// pumps.
#pragma once

#include <stdbool.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

// The five menu labels, already translated by the Rust side (the same
// qbz_i18n keys Linux and macOS use, so the three menus stay identical).
// Must be called before qbz_win_tray_create; the strings are copied.
void qbz_win_tray_set_labels(const wchar_t *play_pause, const wchar_t *next,
                             const wchar_t *previous, const wchar_t *show_hide,
                             const wchar_t *quit);

// Installed once, before create. Each maps 1:1 to a tray_qt dispatcher.
void qbz_win_tray_set_callbacks(void (*on_left_click)(), void (*on_play_pause)(),
                                void (*on_next)(), void (*on_previous)(),
                                void (*on_quit)());

// NIM_ADD + NIM_SETVERSION(4). False if the message window or the icon could
// not be created -- the caller must then leave the tray handle empty, or
// close-to-tray would hide the window with nothing to restore it from.
bool qbz_win_tray_create(const wchar_t *tooltip);

void qbz_win_tray_set_tooltip(const wchar_t *tooltip);
void qbz_win_tray_set_playing(bool playing);
void qbz_win_tray_destroy(void);

#ifdef __cplusplus
}
#endif
