export interface Process {
  pid: number;
  name: string;
  user: string;
  cpu: number;
  memory: number;
  state: string;
}
export interface Overview {
  sampledAt?: number;
  historyError?: string;
  hostname: string;
  os: string;
  kernel: string;
  cores: number;
  uptime: number;
  load: number[];
  cpu: number;
  memoryTotal: number;
  memoryUsed: number;
  swapTotal: number;
  swapUsed: number;
  disks: {
    mount: string;
    device: string;
    total: number;
    used: number;
    available: number;
  }[];
  network: { name: string; received: number; sent: number }[];
  processes: Process[];
  processCount: number;
}
export interface Service {
  name: string;
  active: string;
  sub: string;
  load: string;
  description: string;
}

export type Log = {
  source: "journal" | "service" | "container" | "compose";
  target: string;
  title: string;
};
