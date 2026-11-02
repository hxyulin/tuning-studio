/**
 * A bounded time series. Times are host seconds; a missing or failed read is
 * stored as NaN and drawn as a gap.
 */
export class Series {
  private readonly times: Float64Array;
  private readonly values: Float64Array;
  private start = 0;
  private count = 0;

  constructor(readonly capacity: number) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new RangeError('capacity must be a positive integer');
    }
    this.times = new Float64Array(capacity);
    this.values = new Float64Array(capacity);
  }

  get length(): number {
    return this.count;
  }

  push(time: number, value: number | undefined): void {
    const index = (this.start + this.count) % this.capacity;
    this.times[index] = time;
    this.values[index] = value === undefined || !Number.isFinite(value) ? NaN : value;
    if (this.count < this.capacity) {
      this.count++;
    } else {
      this.start = (this.start + 1) % this.capacity;
    }
  }

  /** Oldest first, with gaps as null so the result survives JSON. */
  snapshot(): { t: number[]; v: (number | null)[] } {
    const t: number[] = [];
    const v: (number | null)[] = [];
    for (let i = 0; i < this.count; i++) {
      const index = (this.start + i) % this.capacity;
      t.push(this.times[index]);
      v.push(Number.isNaN(this.values[index]) ? null : this.values[index]);
    }
    return { t, v };
  }

  clear(): void {
    this.start = 0;
    this.count = 0;
  }
}
