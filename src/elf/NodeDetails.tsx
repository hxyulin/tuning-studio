import { button } from "../ui";
import { type RootNode, type SymbolNode, hex, scalarName, shortLocation } from "./api";
import { MemoryStrip } from "./MemoryStrip";
import { watchable } from "./SymbolTree";

const kindText: Record<SymbolNode["kind"], string> = {
  scalar: "Scalar",
  enum: "Enum",
  taggedEnum: "Enum with data",
  struct: "Struct",
  union: "Union",
  array: "Array",
  pointer: "Pointer (not followed)",
  function: "Function",
  other: "Other",
};

function Field({ name, children }: { name: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[5.5rem_1fr] gap-2.5 py-px">
      <dt className="font-sans text-muted">{name}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}

interface Props {
  node: SymbolNode | null;
  roots: RootNode[];
  onWatch: (node: SymbolNode) => void;
}

export function NodeDetails({ node, roots, onWatch }: Props) {
  if (!node) {
    return <p className="px-3 py-2 text-[12px] text-muted">Select a symbol to see where it lives and how to read it.</p>;
  }
  const root = roots.find((r) => r.path === node.ref.symbol);
  const size = node.size ?? 0;

  return (
    <article className="px-3 py-2 text-[12px]">
      <div className="flex items-start gap-3">
        <h2 className="min-w-0 flex-1 font-mono leading-snug break-all select-text">{node.path}</h2>
        {watchable(node) && (
          <button
            onClick={() => onWatch(node)}
            className={`${button} shrink-0 text-[12px]`}
          >
            {node.expandable ? "Watch numbers inside" : "Watch"}
          </button>
        )}
      </div>
      {!node.readable && (
        <p className="mt-2 text-danger">Cannot be read from the target: {node.status ?? "no valid address"}.</p>
      )}

      <MemoryStrip node={node} roots={roots} />

      <dl className="mt-2 font-mono select-text">
        <Field name="Address">
          {hex(node.address)}
          {size > 1 && <span className="text-muted"> to {hex(node.address + size - 1)}</span>}
        </Field>
        <Field name="Size">{node.size === null ? "unknown" : `${size} ${size === 1 ? "byte" : "bytes"}`}</Field>
        <Field name="Type">{node.typeName}</Field>
        {node.wrapper && <Field name="Wrapped in">{node.wrapper}</Field>}
        <Field name="Kind">{kindText[node.kind]}</Field>
        {node.scalar && <Field name="Read as">{scalarName(node.scalar)}</Field>}
        {node.bitSize !== null && (
          <Field name="Bitfield">
            {node.bitSize} bits at bit {node.bitOffset ?? 0}
          </Field>
        )}
        {node.discrValue !== null && <Field name="Tag value">{node.discrValue}</Field>}
        {node.location && (
          <Field name="Declared at">
            <span title={`${node.location.file}:${node.location.line}`}>{shortLocation(node.location)}</span>
          </Field>
        )}
        {node.childCount !== null && node.kind !== "scalar" && (
          <Field name={node.kind === "array" ? "Elements" : node.kind === "taggedEnum" ? "Entries" : "Members"}>
            {node.childCount}
          </Field>
        )}
        {root && <Field name="Section">{root.section || "none"}</Field>}
      </dl>
    </article>
  );
}
