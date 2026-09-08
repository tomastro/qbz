// Windows notification-area icon, modelled on Qt's own
// qwindowssystemtrayicon.cpp. The whole body is inside `#ifdef _WIN32` so the
// file can sit unconditionally in build.rs beside font_fallback.cpp and
// compile to nothing on Linux and macOS.

#include "win_tray.h"

#ifdef _WIN32

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
// ORDER MATTERS, and it is not alphabetical. <windows.h> must come first:
// <shellapi.h> uses DECLSPEC_IMPORT, HDROP and EXTERN_C without declaring
// them, so including it first produces a wall of C4430/C2146 inside the SDK
// header itself and points nowhere near this file.
#include <windows.h>

#include <shellapi.h>
// GET_X_LPARAM / GET_Y_LPARAM: signed extraction, which matters on a
// multi-monitor desktop where a secondary display has negative coordinates.
#include <windowsx.h>

#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "user32.lib")

namespace {

// WM_APP+101: the tray's callback message. Anything in the WM_APP range is
// ours to define, and this window class receives nothing else.
constexpr UINT kTrayCallback = WM_APP + 101;
constexpr UINT kIconId = 1;

enum MenuId : UINT {
    kMenuPlayPause = 1001,
    kMenuNext,
    kMenuPrevious,
    kMenuShowHide,
    kMenuQuit,
};

HWND g_hwnd = nullptr;
HICON g_icon = nullptr;
// LoadImageW hands us an icon WE own; the LoadIconW(nullptr, ...) fallback is a
// shared system icon that must never be destroyed. Which one we got decides
// whether qbz_win_tray_destroy calls DestroyIcon.
bool g_icon_owned = false;
UINT g_taskbar_created = 0;  // RegisterWindowMessageW("TaskbarCreated")
bool g_playing = false;
wchar_t g_tooltip[128] = L"QBZ";

wchar_t g_label_play_pause[64] = L"Play/Pause";
wchar_t g_label_next[64] = L"Next Track";
wchar_t g_label_previous[64] = L"Previous Track";
wchar_t g_label_show_hide[64] = L"Show/Hide Window";
wchar_t g_label_quit[64] = L"Quit QBZ";

void (*g_on_left_click)() = nullptr;
void (*g_on_play_pause)() = nullptr;
void (*g_on_next)() = nullptr;
void (*g_on_previous)() = nullptr;
void (*g_on_quit)() = nullptr;

void copy_label(wchar_t *dst, size_t cap, const wchar_t *src)
{
    if (!src)
        return;
    wcsncpy_s(dst, cap, src, _TRUNCATE);
}

// The icon Windows draws. winresource assigns id 1 to the first icon it
// embeds (W4), and LoadImageW at 16x16 picks the matching RT_ICON out of that
// group. If it is ever missing we fall back to the generic application icon
// rather than returning null: a tray entry with the wrong picture is a cosmetic
// bug, a tray entry that never appears is close-to-tray hiding the window with
// no way back.
//
// Sets g_icon_owned: without LR_SHARED the LoadImageW handle is OURS and leaks
// unless destroyed, while the LoadIconW fallback is shared and must not be.
HICON load_tray_icon()
{
    HINSTANCE inst = GetModuleHandleW(nullptr);
    const int cx = GetSystemMetrics(SM_CXSMICON);
    const int cy = GetSystemMetrics(SM_CYSMICON);
    if (HICON icon = static_cast<HICON>(
            LoadImageW(inst, MAKEINTRESOURCEW(1), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR))) {
        g_icon_owned = true;
        return icon;
    }
    g_icon_owned = false;
    // MAKEINTRESOURCEW, not IDI_APPLICATION: this build does not define
    // UNICODE, so the bare macro expands to the ANSI MAKEINTRESOURCEA and
    // will not convert to the LPCWSTR LoadIconW wants. 32512 IS
    // IDI_APPLICATION. Every Win32 call in this file is an explicit W for the
    // same reason.
    return LoadIconW(nullptr, MAKEINTRESOURCEW(32512));
}

NOTIFYICONDATAW base_data()
{
    NOTIFYICONDATAW nid = {};
    nid.cbSize = sizeof(nid);
    nid.hWnd = g_hwnd;
    nid.uID = kIconId;
    return nid;
}

bool add_icon()
{
    NOTIFYICONDATAW nid = base_data();
    // NIF_SHOWTIP is REQUIRED with version 4. Version 4 suppresses the standard
    // tooltip unless it is asked for, so without this flag the icon appears,
    // the tooltip is set on every track, and nothing ever shows on hover.
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = kTrayCallback;
    nid.hIcon = g_icon;
    wcsncpy_s(nid.szTip, g_tooltip, _TRUNCATE);
    if (!Shell_NotifyIconW(NIM_ADD, &nid))
        return false;

    // Version 4 is what makes the callback carry the event in LOWORD(lParam)
    // and real screen coordinates in wParam. Without it the right-click
    // arrives as a bare WM_RBUTTONUP with no position and the menu opens in
    // the wrong place on a multi-monitor desktop.
    //
    // The result is CHECKED. wnd_proc decodes callbacks as version 4 and
    // nothing else, so a silent negotiation failure would leave a live icon
    // whose clicks and menu do nothing -- worse than no icon at all, because
    // close-to-tray would then hide the window behind it.
    NOTIFYICONDATAW ver = base_data();
    ver.uVersion = NOTIFYICON_VERSION_4;
    if (!Shell_NotifyIconW(NIM_SETVERSION, &ver)) {
        NOTIFYICONDATAW del = base_data();
        Shell_NotifyIconW(NIM_DELETE, &del);
        return false;
    }
    return true;
}

void show_menu(int x, int y)
{
    HMENU menu = CreatePopupMenu();
    if (!menu)
        return;

    AppendMenuW(menu, MF_STRING, kMenuPlayPause, g_label_play_pause);
    AppendMenuW(menu, MF_STRING, kMenuPrevious, g_label_previous);
    AppendMenuW(menu, MF_STRING, kMenuNext, g_label_next);
    AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
    AppendMenuW(menu, MF_STRING, kMenuShowHide, g_label_show_hide);
    AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
    AppendMenuW(menu, MF_STRING, kMenuQuit, g_label_quit);

    // The documented ritual. Without the SetForegroundWindow the menu does not
    // dismiss when the user clicks elsewhere; without the trailing PostMessage
    // it can stay up after a selection (MSDN TrackPopupMenu remarks).
    SetForegroundWindow(g_hwnd);
    const UINT cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                                    x, y, 0, g_hwnd, nullptr);
    PostMessageW(g_hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    // Hand keyboard focus back to the notification area, which the Shell
    // documentation asks for after a context menu closes -- particularly when
    // it was dismissed with Escape rather than a selection.
    NOTIFYICONDATAW focus = base_data();
    Shell_NotifyIconW(NIM_SETFOCUS, &focus);

    switch (cmd) {
    case kMenuPlayPause:
        if (g_on_play_pause)
            g_on_play_pause();
        break;
    case kMenuNext:
        if (g_on_next)
            g_on_next();
        break;
    case kMenuPrevious:
        if (g_on_previous)
            g_on_previous();
        break;
    case kMenuShowHide:
        if (g_on_left_click)
            g_on_left_click();
        break;
    case kMenuQuit:
        if (g_on_quit)
            g_on_quit();
        break;
    default:
        break;
    }
}

LRESULT CALLBACK tray_wnd_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    // Explorer restarted (or crashed): every icon is gone from the new taskbar
    // and each app has to add its own back. Missing this is why a tray icon
    // "disappears forever" after an Explorer restart.
    if (msg == g_taskbar_created && g_taskbar_created != 0) {
        add_icon();
        return 0;
    }

    if (msg == kTrayCallback) {
        // Version 4 packing: event in the low word of lParam, cursor position
        // in wParam.
        switch (LOWORD(lparam)) {
        case NIN_SELECT:
        case NIN_KEYSELECT:
            if (g_on_left_click)
                g_on_left_click();
            return 0;
        case WM_CONTEXTMENU:
            show_menu(GET_X_LPARAM(wparam), GET_Y_LPARAM(wparam));
            return 0;
        default:
            return 0;
        }
    }

    return DefWindowProcW(hwnd, msg, wparam, lparam);
}

bool ensure_window()
{
    if (g_hwnd)
        return true;

    HINSTANCE inst = GetModuleHandleW(nullptr);
    static const wchar_t *kClass = L"QbzTrayMessageWindow";

    WNDCLASSEXW wc = {};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = tray_wnd_proc;
    wc.hInstance = inst;
    wc.lpszClassName = kClass;
    // Re-registering the same class returns 0 with ERROR_CLASS_ALREADY_EXISTS,
    // which is fine on a second create.
    RegisterClassExW(&wc);

    // NOT HWND_MESSAGE. A message-only window never receives the broadcast
    // TaskbarCreated, so the icon would never come back after an Explorer
    // restart. WS_EX_TOOLWINDOW keeps this invisible helper out of the taskbar
    // and the Alt+Tab list, which is the reason HWND_MESSAGE is tempting.
    g_hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, kClass, L"QbzTrayMessageWindow",
                             WS_OVERLAPPED, 0, 0, 0, 0, nullptr, nullptr, inst, nullptr);
    if (!g_hwnd)
        return false;

    g_taskbar_created = RegisterWindowMessageW(L"TaskbarCreated");
    // A broadcast message is filtered out by UIPI unless the window opts in.
    if (g_taskbar_created)
        ChangeWindowMessageFilterEx(g_hwnd, g_taskbar_created, MSGFLT_ALLOW, nullptr);

    return true;
}

}  // namespace

extern "C" void qbz_win_tray_set_labels(const wchar_t *play_pause, const wchar_t *next,
                                        const wchar_t *previous, const wchar_t *show_hide,
                                        const wchar_t *quit)
{
    copy_label(g_label_play_pause, 64, play_pause);
    copy_label(g_label_next, 64, next);
    copy_label(g_label_previous, 64, previous);
    copy_label(g_label_show_hide, 64, show_hide);
    copy_label(g_label_quit, 64, quit);
}

extern "C" void qbz_win_tray_set_callbacks(void (*on_left_click)(), void (*on_play_pause)(),
                                           void (*on_next)(), void (*on_previous)(),
                                           void (*on_quit)())
{
    g_on_left_click = on_left_click;
    g_on_play_pause = on_play_pause;
    g_on_next = on_next;
    g_on_previous = on_previous;
    g_on_quit = on_quit;
}

extern "C" bool qbz_win_tray_create(const wchar_t *tooltip)
{
    if (tooltip)
        wcsncpy_s(g_tooltip, tooltip, _TRUNCATE);

    // Idempotent: a second create must not stack a second NIM_ADD on a live
    // icon (which fails and would report the whole tray dead while the first
    // icon is still sitting there). Remove the old one first.
    if (g_hwnd) {
        NOTIFYICONDATAW del = base_data();
        Shell_NotifyIconW(NIM_DELETE, &del);
    }

    if (!ensure_window())
        return false;
    if (!g_icon)
        g_icon = load_tray_icon();

    if (add_icon())
        return true;

    // Failed. Give back the window and the icon rather than leaving them
    // parked for the life of the process: the Rust side will not retry, so
    // nothing else would ever free them.
    if (g_icon && g_icon_owned)
        DestroyIcon(g_icon);
    g_icon = nullptr;
    g_icon_owned = false;
    DestroyWindow(g_hwnd);
    g_hwnd = nullptr;
    return false;
}

extern "C" void qbz_win_tray_set_tooltip(const wchar_t *tooltip)
{
    if (!tooltip)
        return;
    // szTip is 128 wchar INCLUDING the terminator; _TRUNCATE keeps the copy
    // safe and Windows simply elides the rest in the hover text.
    wcsncpy_s(g_tooltip, tooltip, _TRUNCATE);
    if (!g_hwnd)
        return;
    NOTIFYICONDATAW nid = base_data();
    // NIF_SHOWTIP again: it is a per-call flag, not sticky state.
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    wcsncpy_s(nid.szTip, g_tooltip, _TRUNCATE);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

extern "C" void qbz_win_tray_set_playing(bool playing)
{
    // Recorded for the menu; the label itself is the platform-neutral
    // "Play/Pause" the other two trays use, so nothing needs rebuilding here.
    g_playing = playing;
}

extern "C" void qbz_win_tray_destroy(void)
{
    if (!g_hwnd)
        return;
    NOTIFYICONDATAW nid = base_data();
    Shell_NotifyIconW(NIM_DELETE, &nid);
    DestroyWindow(g_hwnd);
    g_hwnd = nullptr;
    // LoadImageW WITHOUT LR_SHARED returns an icon this process owns, and it
    // leaks unless destroyed. Only the LoadIconW(nullptr, ...) fallback is a
    // shared system icon, which must never be destroyed.
    if (g_icon && g_icon_owned)
        DestroyIcon(g_icon);
    g_icon = nullptr;
    g_icon_owned = false;
}

#endif  // _WIN32
