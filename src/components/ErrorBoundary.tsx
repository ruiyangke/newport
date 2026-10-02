import { Button } from "./ui/button";
import { Component, Fragment, type ReactNode } from "react";

type Props = { children: ReactNode; scope: "workspace" | "app" };

/** Rendering and lazy-import failures must leave a recovery surface available. */
export class ErrorBoundary extends Component<
  Props,
  { failed: boolean; attempt: number }
> {
  state = { failed: false, attempt: 0 };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (!this.state.failed)
      return (
        <Fragment key={this.state.attempt}>{this.props.children}</Fragment>
      );
    return (
      <section
        data-slot="workspace-recovery"
        className="max-w-[560px] p-[28px]"
        role="alert"
        aria-label="View unavailable"
      >
        <h2 className="mb-[8px] text-[16px]">
          {this.props.scope === "workspace"
            ? "This workspace couldn’t be displayed"
            : "Newport couldn’t display this window"}
        </h2>
        <p className="mb-[16px] text-muted-foreground">
          {this.props.scope === "workspace"
            ? "Try opening it again, or choose another workspace from the sidebar."
            : "Reload the window to try again."}
        </p>
        <div className="flex gap-[8px]">
          <Button
            variant="outline"
            className="rounded-[5px] border border-border px-[10px] py-[5px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            type="button"
            onClick={() =>
              this.setState(({ attempt }) => ({
                failed: false,
                attempt: attempt + 1,
              }))
            }
          >
            Try again
          </Button>
          <Button
            variant="outline"
            className="rounded-[5px] border border-border px-[10px] py-[5px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            type="button"
            onClick={() => window.location.reload()}
          >
            Reload window
          </Button>
        </div>
      </section>
    );
  }
}
