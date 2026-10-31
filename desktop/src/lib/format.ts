export const gb = (mb: number, digits = 1) => (mb / 1024).toFixed(digits);
export const pct = (part: number, whole: number) => (whole <= 0 ? 0 : Math.min(100, (part / whole) * 100));
export const uid = () => Math.random().toString(36).slice(2, 10);
