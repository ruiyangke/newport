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
        className="workspace-recovery"
        role="alert"
        aria-label="View unavailable"
      >
        <h2>
          {this.props.scope === "workspace"
            ? "This workspace couldn’t be displayed"
            : "Porthop couldn’t display this window"}
        </h2>
        <p>
          {this.props.scope === "workspace"
            ? "Try opening it again, or choose another workspace from the sidebar."
            : "Reload the window to try again."}
        </p>
        <div>
          <Button
            variant="outline"
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
