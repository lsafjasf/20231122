#!/usr/bin/env python3
"""生成淡化检查的测试夹具（仅测试辅助，不属于应用代码）。

输出 16-bit PCM WAV：
  - fade_both.wav   首尾线性淡化，应判“淡入+淡出”
  - no_fade.wav     恒定电平，应判“无淡化”
  - boundary.wav    端部 RMS 比值落在边界带内，应判“无法判断”
另外由调用方复制一份未知扩展文件与缺失路径用于来源检查。
"""
import math
import os
import struct
import wave

RATE = 48_000
DURATION = 2.0
N = int(RATE * DURATION)


def write_wav(path, samples):
    with wave.open(path, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        frames = b"".join(struct.pack("<h", max(-32767, min(32767, int(s * 32767)))) for s in samples)
        wav.writeframes(frames)


def tone(n):
    return [0.6 * math.sin(2 * math.pi * 440 * i / RATE) for i in range(n)]


def apply_fades(base, head_frac=0.05, tail_frac=0.05, depth=1.0):
    """depth=1 完全淡化到 0；depth 越小端部电平越高，用于构造边界样本。"""
    out = list(base)
    h = int(len(out) * head_frac)
    t = int(len(out) * tail_frac)
    for i in range(h):
        gain = (1 - depth) + depth * (i / max(1, h - 1))
        out[i] *= gain
    for i in range(t):
        gain = (1 - depth) + depth * (i / max(1, t - 1))
        out[len(out) - 1 - i] *= gain
    return out


def main(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    base = tone(N)
    write_wav(os.path.join(out_dir, "fade_both.wav"), apply_fades(base, depth=1.0))
    write_wav(os.path.join(out_dir, "no_fade.wav"), base)
    # 端部保留约 52% 电平：RMS 比值落在 (0.45, 0.60) 边界带内。
    write_wav(os.path.join(out_dir, "boundary.wav"), apply_fades(base, depth=0.48))


if __name__ == "__main__":
    import sys
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
