# 앱 아이콘 원본(1024x1024 PNG)을 외부 라이브러리 없이 그린다.
# 둥근 사각형 배경 + 두 화면을 잇는 레가토 곡선(이음줄).
import math, struct, sys, zlib

N = 1024
px = bytearray(N * N * 4)

def blend(x, y, r, g, b, a):
    if not (0 <= x < N and 0 <= y < N) or a <= 0:
        return
    i = (y * N + x) * 4
    da = px[i + 3] / 255
    sa = min(a, 1.0)
    oa = sa + da * (1 - sa)
    for k, c in enumerate((r, g, b)):
        px[i + k] = int((c * sa + px[i + k] * da * (1 - sa)) / oa) if oa else 0
    px[i + 3] = int(oa * 255)

def rounded_rect_alpha(x, y, x0, y0, x1, y1, rad):
    cx = min(max(x, x0 + rad), x1 - rad)
    cy = min(max(y, y0 + rad), y1 - rad)
    d = math.hypot(x - cx, y - cy) - rad
    return max(0.0, min(1.0, 0.5 - d))

# 배경: 남색 → 보라 그라데이션 둥근 사각형
for y in range(N):
    for x in range(N):
        a = rounded_rect_alpha(x + 0.5, y + 0.5, 64, 64, 960, 960, 200)
        if a > 0:
            t = (x + y) / (2 * N)
            r = int(30 + 60 * t); g = int(40 + 50 * t); b = int(90 + 120 * t)
            blend(x, y, r, g, b, a)

# 두 화면 (테두리 사각형)
def frame(x0, y0, x1, y1, th, col):
    for y in range(int(y0) - 2, int(y1) + 3):
        for x in range(int(x0) - 2, int(x1) + 3):
            outer = rounded_rect_alpha(x + 0.5, y + 0.5, x0, y0, x1, y1, 36)
            inner = rounded_rect_alpha(x + 0.5, y + 0.5, x0 + th, y0 + th, x1 - th, y1 - th, 36 - th / 2)
            a = outer - inner
            if a > 0:
                blend(x, y, *col, a)

frame(170, 300, 470, 520, 34, (232, 236, 255))
frame(554, 504, 854, 724, 34, (232, 236, 255))

# 이음줄: 두 화면을 잇는 두꺼운 곡선 (2차 베지어)
p0, p1, p2 = (320, 262), (660, 40), (704, 470)
steps = 1400
for s in range(steps + 1):
    t = s / steps
    bx = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t ** 2 * p2[0]
    by = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t ** 2 * p2[1]
    rad = 22
    for y in range(int(by - rad - 1), int(by + rad + 2)):
        for x in range(int(bx - rad - 1), int(bx + rad + 2)):
            d = math.hypot(x + 0.5 - bx, y + 0.5 - by)
            a = max(0.0, min(1.0, rad - d + 0.5))
            if a > 0:
                i = (y * N + x) * 4
                # 같은 곡선 위 점이 여러 번 덮어도 진해지지 않게 최대값만 쓴다.
                cur = px[i + 3] / 255 if px[i] == 124 and px[i + 1] == 156 else 0
                if a > cur:
                    blend(x, y, 124, 156, 255, a)

raw = b''.join(b'\x00' + bytes(px[y * N * 4:(y + 1) * N * 4]) for y in range(N))
def chunk(t, d):
    return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', N, N, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
