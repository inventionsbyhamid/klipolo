export function usToPx(us: number, pps: number): number {
  return (us / 1_000_000) * pps;
}

export function pxToUs(px: number, pps: number): number {
  return (px / pps) * 1_000_000;
}
