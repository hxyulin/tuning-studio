import { type RootNode, type SymbolNode, hex } from "./api";

/** Where `node` sits among the statics sharing its 16 MiB address region. */
export function MemoryStrip({ node, roots }: { node: SymbolNode; roots: RootNode[] }) {
  const region = Math.floor(node.address / 0x100_0000);
  const neighbours = roots.filter((r) => Math.floor(r.address / 0x100_0000) === region && (r.size ?? 0) > 0);
  if (neighbours.length === 0) return null;

  const start = Math.min(...neighbours.map((r) => r.address), node.address);
  const end = Math.max(...neighbours.map((r) => r.address + (r.size ?? 0)), node.address + (node.size ?? 1));
  const span = Math.max(end - start, 1);
  const x = (addr: number) => ((addr - start) / span) * 1000;
  const w = (size: number | null) => Math.max(((size ?? 0) / span) * 1000, 1.5);

  return (
    <figure className="mt-2">
      <svg viewBox="0 0 1000 28" preserveAspectRatio="none" className="h-7 w-full" role="img"
        aria-label={`${node.label} at ${hex(node.address)} among ${neighbours.length} statics from ${hex(start)} to ${hex(end)}`}>
        <rect x="0" y="6" width="1000" height="16" fill="var(--sunken)" />
        {neighbours.map((r) => (
          <rect key={r.path + r.address} x={x(r.address)} y="6" width={w(r.size)} height="16" fill="var(--rule)" />
        ))}
        <rect x={x(node.address)} y="2" width={w(node.size)} height="24" fill="var(--accent)" />
      </svg>
      <figcaption className="mt-1 flex justify-between font-mono text-[11px] text-muted">
        <span>{hex(start)}</span>
        <span>{neighbours.length} statics in this region</span>
        <span>{hex(end)}</span>
      </figcaption>
    </figure>
  );
}
