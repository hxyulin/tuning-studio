// The Live Watch tree for the simulated target, shaped like the DWARF view of `Control`.
import { type Axis, type Motor, type Pid, control } from "./sim";

export interface Node {
  id: string;
  name: string;
  type: string;
  children?: Node[];
  /** A number to plot, or text for enums and options */
  read?: () => number | string;
  unit?: string;
  write?: (value: number) => void;
}

function scalar<T extends object>(parent: string, owner: () => T, key: keyof T & string, type: string, unit?: string, writable = false): Node {
  return {
    id: `${parent}.${key}`,
    name: key,
    type,
    unit,
    read: () => owner()[key] as number,
    write: writable ? (v) => ((owner()[key] as number) = v) : undefined,
  };
}

function pid(parent: string, owner: () => Pid, units: [string, string]): Node {
  const id = parent;
  const [input, output] = units;
  return {
    id,
    name: id.split(".").pop()!,
    type: "Pid",
    children: [
      scalar(id, owner, "kp", "f32", `${output}/${input}`, true),
      scalar(id, owner, "ki", "f32", `${output}/(${input}·s)`, true),
      scalar(id, owner, "kd", "f32", `${output}·s/${input}`, true),
      scalar(id, owner, "maxout", "f32", output, true),
      scalar(id, owner, "integral", "f32", output),
      scalar(id, owner, "last_error", "f32", input),
    ],
  };
}

function axis(name: "yaw" | "pitch"): Node {
  const id = `control.${name}`;
  const owner = (): Axis => control[name];
  return {
    id,
    name,
    type: "Axis",
    children: [
      scalar(id, owner, "target_angle_rad", "f32", "rad"),
      pid(`${id}.angle_pid`, () => owner().angle_pid, ["rad", "rpm"]),
      pid(`${id}.speed_pid`, () => owner().speed_pid, ["rpm", "A"]),
    ],
  };
}

function motor(index: number): Node {
  const id = `control.motors[${index}]`;
  const owner = () => control.motors[index] as Motor;
  const present = control.motors[index] !== null;
  return {
    id,
    name: `[${index}]`,
    type: "Option<Motor>",
    read: () => (present ? "Some" : "None"),
    children: present
      ? [
          scalar(id, owner, "angle_rad", "f32", "rad"),
          scalar(id, owner, "speed_rpm", "f32", "rpm"),
          scalar(id, owner, "current_a", "f32", "A"),
          scalar(id, owner, "target_rpm", "f32", "rpm"),
          scalar(id, owner, "command_current_a", "f32", "A"),
        ]
      : undefined,
  };
}

const remote = () => control.remote;

export const roots: Node[] = [
  {
    id: "control",
    name: "control",
    type: "Control",
    children: [
      { id: "control.state", name: "state", type: "State", read: () => control.state },
      scalar("control", () => control, "tick", "u32"),
      {
        id: "control.remote",
        name: "remote",
        type: "Option<Dr16>",
        read: () => "Some",
        children: [
          {
            id: "control.remote.channels",
            name: "channels",
            type: "[i16; 4]",
            children: [0, 1, 2, 3].map((i) => ({
              id: `control.remote.channels[${i}]`,
              name: `[${i}]`,
              type: "i16",
              read: () => remote().channels[i],
            })),
          },
          scalar("control.remote", remote, "left_switch", "u8"),
          scalar("control.remote", remote, "right_switch", "u8"),
        ],
      },
      { id: "control.motors", name: "motors", type: "[Option<Motor>; 4]", children: [0, 1, 2, 3].map(motor) },
      axis("yaw"),
      axis("pitch"),
    ],
  },
];

export const byId = new Map<string, Node>();
(function index(nodes: Node[]) {
  for (const n of nodes) {
    byId.set(n.id, n);
    if (n.children) index(n.children);
  }
})(roots);

/** Every numeric leaf, so a newly plotted value comes with its history */
export const numeric = [...byId.values()].filter((n) => n.read && !n.children && typeof n.read() === "number");
