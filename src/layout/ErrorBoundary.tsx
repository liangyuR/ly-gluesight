import { Component, type ReactNode } from "react";
import { CircleAlert } from "lucide-react";
import { Button, Card, EmptyState } from "qing-steel";

/** 页面渲染出错时只把这一页换成报错，侧栏和别的页面照常能用。 */
export default class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <Card className="page-error" role="alert">
        <EmptyState
          icon={<CircleAlert aria-hidden="true" />}
          title="页面出错了"
          description={<span className="mono">{this.state.error.message}</span>}
          action={<Button onClick={() => this.setState({ error: null })}>重试</Button>}
        />
      </Card>
    );
  }
}
