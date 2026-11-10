# EdenVoice UI Design System

EdenVoice uses a quiet, voice-first desktop language: clear hierarchy, restrained
color, compact controls, and enough whitespace to scan channels while playing. It is
Apple-inspired in typography, rhythm, and restraint, but follows the native behavior
of Windows, macOS, and Linux rather than imitating one operating system.

## Principles

- Voice state comes first. Connection, current channel, speaking, mute, deafen, and
  audio health must be understandable without opening diagnostics.
- Color is semantic. Accent indicates selection/action; success, warning, and danger
  are reserved for their named states.
- Selection and hover are subtle. Speaking uses a small indicator and label rather
  than a flashing or fully colored row.
- Technical detail stays in Settings diagnostics, not the everyday channel view.
- Controls use honest capability and loading states. Unsupported options are not
  simulated, and background DSP initialization never masquerades as ready.

## Tokens

The executable definitions live in `client/src/ui/theme.rs`.

### Color roles

`background`, `surface`, `surface_secondary`, `surface_hover`, `surface_selected`,
and `separator` define structure. `text_primary`, `text_secondary`, and `text_muted`
define hierarchy. `accent`, `success`, `warning`, and `danger` carry semantic state.
Light and dark palettes are designed independently; System follows the OS preference.

### Spacing and radius

Spacing uses one scale: 4, 8, 12, 16, 20, 24, and 32 px. Corner radii use 6, 10,
14, and 18 px. New UI should use these constants instead of introducing one-off
values unless a platform API requires a fixed dimension.

### Type hierarchy

- Caption: secondary status and supporting copy.
- Body: controls, users, and ordinary content.
- Body Strong: labels and selected values.
- Title: settings sections and channel headings.
- Large Title: product/connection entry point.

### Motion

Fast, normal, and slow durations are 120, 180, and 240 ms. Motion communicates state
change only. It must not schedule work on the audio thread or continuously repaint an
otherwise idle window. Speaking transitions should remain within 150–250 ms.

## Information architecture

The main window keeps channels and members visible, with connection state at the top
and voice controls in a persistent bottom bar. A user popover owns per-user volume
and local mute. Settings uses separate Appearance, Audio, and Diagnostics pages; the
latter holds network/DSP detail that would distract in the main view.

## Component behavior

- Channel rows: full-width target, light hover, restrained selected surface.
- User rows: small speaking dot/halo, name, and a secondary Speaking label.
- Mute/deafen: explicit labels and persistent state; deafen also implies local mute
  through the existing server semantics.
- Gain: 0–200% input and per-user ranges where supported, with a visible 100% reset.
- Destructive/error states: semantic danger color and plain-language recovery text.
- Loading: a spinner plus the processor being prepared; the current processor remains
  identified separately.

## Accessibility and cross-platform rules

Text remains the primary state carrier; color is supplementary. Controls retain
keyboard focus behavior supplied by egui, text and hit targets remain legible under
HiDPI scaling, and System theme is the default. Platform-specific shortcuts or
permissions must include a textual fallback/error. Avoid hard-coded assumptions about
macOS fonts, title bars, device naming, or filesystem paths.

## Review checklist

- Uses semantic palette and spacing tokens.
- Does not add a card/pill for every element.
- Shows hover, selected, disabled, loading, error, and empty states where relevant.
- Keeps core/network/audio operations behind `AppCommand` and `AppState`.
- Does not cause continuous idle repaint or perform work on realtime audio callbacks.
