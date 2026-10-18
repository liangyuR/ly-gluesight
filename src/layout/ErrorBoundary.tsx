import { Component, type ReactNode } from "react";
import { CircleAlert } from "lucide-react";

/** 页面渲染出错时只把这一页换成报错，侧栏和别的页面照常能用。 */
export default class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="panel page-error" role="alert">
        <div className="qs-empty">
          <CircleAlert />
          <b>页面出错了</b>
          <small className="mono">{this.state.error.message}</small>
          <button className="btn" onClick={() => this.setState({ error: null })}>
            重试
          </button>
        </div>
      </div>
    );
  }
}
