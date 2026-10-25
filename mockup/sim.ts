// A stand-in target: the training template's gimbal, with a cascaded angle/speed PID per axis,
// stepped at 1 kHz. Only the mockup uses it.

export type State = "Waiting" | "Prepare" | "Running" | "Error";

export interface Pid {
  kp: number;
  ki: number;
  kd: number;
  maxout: number;
  integral: number;
  last_error: number;
}

export interface Motor {
  angle_rad: number;
  speed_rpm: number;
  current_a: number;
  target_rpm: number;
  command_current_a: number;
}

export interface Axis {
  target_angle_rad: number;
  angle_pid: Pid;
  speed_pid: Pid;
}

const pid = (kp: number, ki: number, kd: number, maxout: number): Pid => ({ kp, ki, kd, maxout, integral: 0, last_error: 0 });
const motor = (): Motor => ({ angle_rad: 0, speed_rpm: 0, current_a: 0, target_rpm: 0, command_current_a: 0 });

export const control = {
  state: "Running" as State,
  tick: 0,
  remote: { channels: [0, 0, 0, 0], left_switch: 3, right_switch: 3 },
  motors: [motor(), motor(), null, null] as (Motor | null)[],
  yaw: { target_angle_rad: 0, angle_pid: pid(200, 0, 0, 120), speed_pid: pid(0.05, 0.5, 0, 3) } as Axis,
  pitch: { target_angle_rad: 0, angle_pid: pid(160, 0, 0, 90), speed_pid: pid(0.04, 0.4, 0, 3) } as Axis,
};

const DT = 0.001;
const KT = 0.741; // GM6020, N·m/A
const RPM = 60 / (2 * Math.PI);
const plant = [
  { inertia: 0.02, friction: 0.004, gravity: 0, omega: 0 },
  { inertia: 0.012, friction: 0.003, gravity: 0.08, omega: 0 },
];

function calculate(p: Pid, target: number, measured: number) {
  const error = target - measured;
  p.integral = Math.max(-p.maxout, Math.min(p.maxout, p.integral + p.ki * error * DT));
  const out = p.kp * error + p.integral + (p.kd * (error - p.last_error)) / DT;
  p.last_error = error;
  return Math.max(-p.maxout, Math.min(p.maxout, out));
}

function noise(scale: number) {
  return (Math.random() - 0.5) * scale;
}

export function step() {
  control.tick++;
  const t = control.tick * DT;
  // The operator flicks the stick: yaw and pitch step back and forth, out of phase
  control.yaw.target_angle_rad = Math.floor(t / 2.5) % 2 ? 0.6 : -0.6;
  control.pitch.target_angle_rad = Math.floor((t + 1.25) / 2.5) % 2 ? 0.25 : -0.1;
  control.remote.channels[2] = Math.round(Math.sin(t * 2.1) * 40);
  control.remote.channels[3] = Math.round(Math.cos(t * 1.7) * 25);

  [control.yaw, control.pitch].forEach((axis, i) => {
    const m = control.motors[i]!;
    const body = plant[i];
    const running = control.state === "Running";
    m.target_rpm = running ? calculate(axis.angle_pid, axis.target_angle_rad, m.angle_rad) : 0;
    m.command_current_a = running ? calculate(axis.speed_pid, m.target_rpm, m.speed_rpm) : 0;
    m.current_a += (m.command_current_a - m.current_a) * 0.2 + noise(0.02);
    const torque = KT * m.current_a - body.friction * body.omega * RPM - body.gravity * Math.sin(m.angle_rad);
    body.omega += (torque / body.inertia) * DT;
    m.angle_rad += body.omega * DT;
    m.speed_rpm = body.omega * RPM + noise(1.5);
  });
}
