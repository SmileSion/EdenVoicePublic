export type UiSound = "message" | "room-joined" | "room-left" | "admin-muted" | "timer-done";

type WebAudioWindow = Window & typeof globalThis & {
  webkitAudioContext?: typeof AudioContext;
};

let audioContext: AudioContext | null = null;

const patterns: Record<UiSound, Array<{ frequency: number; duration: number; delay: number }>> = {
  "message": [{frequency:740,duration:0.08,delay:0}],
  "timer-done": [{ frequency: 1046.5, duration: 0.65, delay: 0 }],
  "room-joined": [
    { frequency: 440, duration: 0.08, delay: 0 },
    { frequency: 660, duration: 0.12, delay: 0.075 },
  ],
  "room-left": [
    { frequency: 560, duration: 0.08, delay: 0 },
    { frequency: 350, duration: 0.14, delay: 0.075 },
  ],
  "admin-muted": [
    { frequency: 260, duration: 0.1, delay: 0 },
    { frequency: 190, duration: 0.18, delay: 0.09 },
  ],
};

export async function prepareUiAudio() {
  const AudioContextClass = window.AudioContext || (window as WebAudioWindow).webkitAudioContext;
  if (!AudioContextClass) return;
  audioContext ??= new AudioContextClass();
  if (audioContext.state === "suspended") await audioContext.resume().catch(() => undefined);
}

/** Short synthesized cues avoid shipping or decoding external audio assets. */
export async function playUiSound(sound: UiSound, outputVolume = 1) {
  const AudioContextClass = window.AudioContext || (window as WebAudioWindow).webkitAudioContext;
  if (!AudioContextClass || outputVolume <= 0) return;
  audioContext ??= new AudioContextClass();
  if (audioContext.state === "suspended") await audioContext.resume().catch(() => undefined);
  if (audioContext.state !== "running") return;

  const start = audioContext.currentTime + 0.012;
  const level = Math.min(1, outputVolume) * (sound === "admin-muted" ? 0.052 : 0.04);
  patterns[sound].forEach(({ frequency, duration, delay }) => {
    const oscillator = audioContext!.createOscillator();
    const gain = audioContext!.createGain();
    const toneStart = start + delay;
    const toneEnd = toneStart + duration;
    oscillator.type = "sine";
    oscillator.frequency.setValueAtTime(frequency, toneStart);
    gain.gain.setValueAtTime(0.0001, toneStart);
    gain.gain.exponentialRampToValueAtTime(level, toneStart + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, toneEnd);
    oscillator.connect(gain).connect(audioContext!.destination);
    oscillator.start(toneStart);
    oscillator.stop(toneEnd + 0.01);
  });
}
