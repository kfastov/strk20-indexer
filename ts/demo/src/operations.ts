export interface Operation {
  label: string;
  started: number;
  elapsedMs?: number;
  error?: string;
  detail?: string;
  children: Operation[];
}

/** One timing source for the screen, exported benchmark and video. */
export class Operations {
  readonly entries: Operation[] = [];
  active: Operation | undefined;
  detail(name: string, ms: number): void {
    this.active?.children.push({
      label: name,
      started: performance.now() - ms,
      elapsedMs: ms,
      children: [],
    });
  }
  private readonly changed: () => void;
  constructor(changed: () => void) {
    this.changed = changed;
  }

  async run<T>(
    label: string,
    action: (operation: Operation) => Promise<T>,
    parent?: Operation,
  ): Promise<T> {
    const operation: Operation = {
      label,
      started: performance.now(),
      children: [],
    };
    ((parent ?? this.active)?.children ?? this.entries).push(operation);
    const previous = this.active;
    this.active = operation;
    this.changed();
    try {
      return await action(operation);
    } catch (error) {
      operation.error =
        error instanceof Error ? error.message : "Operation failed.";
      throw error;
    } finally {
      operation.elapsedMs = performance.now() - operation.started;
      this.active = previous;
      this.changed();
    }
  }
}
