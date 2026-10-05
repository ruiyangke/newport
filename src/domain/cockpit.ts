export interface Process {
  pid: number;
  name: string;
  user: string;
  cpu: number;
  memory: number;
  state: string;
  /** The command line, when the process has one (kernel threads do not). */
  command?: string;
}
export interface Overview {
  sampledAt?: number;
  historyError?: string;
  collectionError?: string;
  hostname: string;
  os: string;
  kernel: string;
  cores: number;
  uptime: number;
  load: number[];
  cpu: number;
  /**
   * Each online core's usage by the kernel's id, with the agent's last minute
   * (one value per second, oldest first).
   */
  cpus?: { id: number; usage: number; history?: number[] }[];
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

export type Log = {
  source: "container" | "compose";
  target: string;
  title: string;
};
