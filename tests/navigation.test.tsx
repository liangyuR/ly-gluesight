import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import App from "../src/App";
import ErrorBoundary from "../src/layout/ErrorBoundary";
import { navEntries, navItems, isNavGroup } from "../src/layout/nav";

describe("应用路由与错误恢复", () => {
  it.each(["/", "/unknown-route"])("%s 进入操作流程引导", async path => {
    render(<MemoryRouter initialEntries={[path]}><App /></MemoryRouter>);
    expect(await screen.findByRole("heading", { level: 1, name: "操作流程" })).toBeVisible();
    expect(within(screen.getByRole("navigation", { name: "操作导航" })).getByRole("link", { name: "操作流程" })).toHaveAttribute("aria-current", "page");
  });

  it("历史复测匹配具体路由，导航切到历史列表后更新选中项", async () => {
    render(<MemoryRouter initialEntries={["/history/retest"]}><App /></MemoryRouter>);
    const nav = within(screen.getByRole("navigation", { name: "操作导航" }));
    expect(await screen.findByRole("heading", { level: 1, name: "历史复测" })).toBeVisible();
    expect(nav.getByRole("link", { name: "历史复测" })).toHaveAttribute("aria-current", "page");
    expect(nav.getByRole("link", { name: "历史记录" })).not.toHaveAttribute("aria-current");
    await userEvent.click(nav.getByRole("link", { name: "历史记录" }));
    expect(screen.getByRole("heading", { level: 1, name: "历史记录" })).toBeVisible();
    expect(nav.getByRole("link", { name: "历史记录" })).toHaveAttribute("aria-current", "page");
  });

  it("侧栏可折叠再展开，浏览器模式提示可见", async () => {
    render(<MemoryRouter initialEntries={["/history/retest"]}><App /></MemoryRouter>);
    await userEvent.click(screen.getByRole("button", { name: "折叠侧边栏" }));
    expect(document.querySelector(".sidebar")).toHaveClass("collapsed");
    await userEvent.click(screen.getByRole("button", { name: "展开侧边栏" }));
    expect(document.querySelector(".sidebar")).not.toHaveClass("collapsed");
    expect(screen.getByText("浏览器查看模式：设备、配方保存与生产操作需要桌面后端。")).toBeVisible();
    const nav=within(screen.getByRole("navigation",{name:"操作导航"}));
    for(const item of navItems)expect(nav.getByRole("link",{name:item.label})).toHaveAttribute("href",item.path);
  });

  it.each(navItems.map(item=>[item.path,item.label]))("直接打开 %s 显示 %s 且只选中对应导航",async(path,label)=>{
    render(<MemoryRouter initialEntries={[path]}><App/></MemoryRouter>);
    expect(await screen.findByRole("heading",{level:1,name:label})).toBeVisible();
    const nav=within(screen.getByRole("navigation",{name:"操作导航"}));
    expect(nav.getByRole("link",{name:label})).toHaveAttribute("aria-current","page");
    expect(nav.getAllByRole("link").filter(link=>link.hasAttribute("aria-current"))).toHaveLength(1);
  });

  it("导航包含所有分组及唯一的对应路由，折叠状态仍保留可访问名称和可点击链接",async()=>{
    render(<MemoryRouter initialEntries={["/guide"]}><App/></MemoryRouter>);
    const nav=within(screen.getByRole("navigation",{name:"操作导航"}));
    for(const group of navEntries.filter(isNavGroup))expect(nav.getByText(group.label)).toBeVisible();
    expect(new Set(navItems.map(item=>item.path)).size).toBe(navItems.length);
    await userEvent.click(screen.getByRole("button",{name:"折叠侧边栏"}));
    for(const item of navItems)expect(nav.getByRole("link",{name:item.label})).toHaveAttribute("title",item.label);
    await userEvent.click(nav.getByRole("link",{name:"工件总览"}));
    expect(screen.getByRole("heading",{level:1,name:"工件总览"})).toBeVisible();
    expect(nav.getByRole("link",{name:"工件总览"})).toHaveAttribute("aria-current","page");
  });

  it("历史详情高亮历史记录，路径前缀相似的未知地址正确回退引导",async()=>{
    const page=render(<MemoryRouter initialEntries={["/history/7"]}><App/></MemoryRouter>);
    expect(within(screen.getByRole("navigation",{name:"操作导航"})).getByRole("link",{name:"历史记录"})).toHaveAttribute("aria-current","page");
    page.unmount();render(<MemoryRouter initialEntries={["/camera-wrong"]}><App/></MemoryRouter>);
    expect(await screen.findByRole("heading",{level:1,name:"操作流程"})).toBeVisible();
  });

  it("页面渲染异常显示原因，修复后可重试", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    let broken = true;
    function Content() { if (broken) throw new Error("图像显示异常"); return <p>页面恢复</p>; }
    render(<ErrorBoundary><Content /></ErrorBoundary>);
    expect(screen.getByText("页面出错了")).toBeVisible(); expect(screen.getByText("图像显示异常")).toBeVisible();
    expect(errors).toHaveBeenCalled(); broken = false;
    await userEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(screen.getByText("页面恢复")).toBeVisible();
  });
});
