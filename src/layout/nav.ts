import type { ComponentType } from "react";
import { BookOpen, Cable, Camera, Crosshair, FlaskConical, History, LayoutDashboard, ScanEye, ScrollText, Settings, type LucideIcon } from "lucide-react";
import InspectPage from "../pages/InspectPage";
import CameraPage from "../pages/CameraPage";
import FlyshotCalibrationPage from "../pages/FlyshotCalibrationPage";
import { PlcLogsPage, PlcSettingsPage } from "../pages/PlcPages";
import HistoryPage from "../pages/HistoryPage";
import SettingsPage from "../pages/SettingsPage";
import OperationGuidePage from "../pages/OperationGuidePage";
import OfflinePage from "../pages/OfflinePage";
import LibraryPage from "../features/workspace/LibraryPage";
import ProgressPage from "../features/workspace/ProgressPage";
import CapturePage from "../features/workspace/CapturePage";
import GeometryPage from "../features/workspace/GeometryPage";
import TeachingPage from "../features/workspace/TeachingPage";
import OverviewPage from "../features/workspace/OverviewPage";
import ValidationPage from "../features/workspace/ValidationPage";

export interface PageRoute {
  path: string;
  label: string;
  element: ComponentType;
}

export interface NavItem extends PageRoute {
  icon: LucideIcon;
}

export interface NavGroup {
  label: string;
  children: NavItem[];
}

export type NavEntry = NavItem | NavGroup;

export function isNavGroup(entry: NavEntry): entry is NavGroup {
  return "children" in entry;
}

const recipe: NavItem = { path:"/recipe",label:"配方",icon:BookOpen,element:LibraryPage };

export const settingsItem: NavItem = { path:"/settings",label:"系统设置",icon:Settings,element:SettingsPage };

export const navEntries: NavEntry[] = [
  { path:"/guide",label:"总览",icon:LayoutDashboard,element:OperationGuidePage },
  { label:"生产",children:[
    { path:"/inspect",label:"在线检测",icon:ScanEye,element:InspectPage },
    { path:"/history",label:"历史记录",icon:History,element:HistoryPage },
  ] },
  { label:"工艺",children:[
    recipe,
    { path:"/offline",label:"离线模式",icon:FlaskConical,element:OfflinePage },
  ] },
  { label:"设备",children:[
    { path:"/camera",label:"相机与采集",icon:Camera,element:CameraPage },
    { path:"/camera/calibration",label:"飞拍标定",icon:Crosshair,element:FlyshotCalibrationPage },
    { path:"/plc",label:"PLC 通讯",icon:Cable,element:PlcSettingsPage },
    { path:"/plc-logs",label:"通讯日志",icon:ScrollText,element:PlcLogsPage },
  ] },
];

export const navItems: NavItem[] = navEntries.flatMap((entry) => (isNavGroup(entry) ? entry.children : [entry]));

const recipeSteps: PageRoute[] = [
  { path:"/recipe/progress",label:"进度总览",element:ProgressPage },
  { path:"/recipe/capture",label:"整圈采集",element:CapturePage },
  { path:"/recipe/geometry",label:"配方默认参数",element:GeometryPage },
  { path:"/recipe/teach",label:"单帧示教",element:TeachingPage },
  { path:"/recipe/overview",label:"工件总览",element:OverviewPage },
  { path:"/recipe/validation",label:"验证与发布",element:ValidationPage },
].map(step => ({ ...step, label:`${recipe.label} · ${step.label}` }));

export const pageRoutes: PageRoute[] = [...navItems, ...recipeSteps, settingsItem];

export function matchRoute<T extends PageRoute>(routes: T[], pathname: string): T | undefined {
  return routes.filter(route => pathname === route.path || pathname.startsWith(route.path + "/")).sort((a, b) => b.path.length - a.path.length)[0];
}
