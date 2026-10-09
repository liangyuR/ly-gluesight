import type { ComponentType } from "react";
import { Cable, Camera, History, ScanEye, ScrollText, Settings, SlidersHorizontal, type LucideIcon } from "lucide-react";
import InspectPage from "../pages/InspectPage";
import CameraPage, { FlyshotCalibrationPage } from "../pages/CameraPage";
import { PlcLogsPage, PlcSettingsPage } from "../pages/PlcPages";
import HistoryPage from "../pages/HistoryPage";
import SettingsPage from "../pages/SettingsPage";
import OperationGuidePage from "../pages/OperationGuidePage";
import HistoryRetestPage from "../pages/HistoryRetestPage";
import LibraryPage from "../features/workspace/LibraryPage";
import GeometryPage from "../features/workspace/GeometryPage";
import TeachingPage from "../features/workspace/TeachingPage";
import OverviewPage from "../features/workspace/OverviewPage";
import ValidationPage from "../features/workspace/ValidationPage";

export interface NavItem {
  path: string;
  label: string;
  icon: LucideIcon;
  element: ComponentType;
}

export interface NavGroup {
  label: string;
  icon: LucideIcon;
  children: NavItem[];
}

export type NavEntry = NavItem | NavGroup;

export function isNavGroup(entry: NavEntry): entry is NavGroup {
  return "children" in entry;
}

export const navEntries: NavEntry[] = [
  { path:"/guide",label:"操作流程",icon:ScanEye,element:OperationGuidePage },
  { label:"生产与复盘",icon:ScanEye,children:[
    { path:"/inspect",label:"在线检测",icon:ScanEye,element:InspectPage },
    { path:"/history",label:"历史记录",icon:History,element:HistoryPage },
    { path:"/history/retest",label:"历史复测",icon:History,element:HistoryRetestPage },
  ] },
  { label:"配方工作台",icon:SlidersHorizontal,children:[
    { path:"/recipe",label:"配方库",icon:SlidersHorizontal,element:LibraryPage },
    { path:"/recipe/geometry",label:"胶路与拍照规划",icon:SlidersHorizontal,element:GeometryPage },
    { path:"/recipe/teach",label:"单帧示教",icon:ScanEye,element:TeachingPage },
    { path:"/recipe/overview",label:"工件总览",icon:ScanEye,element:OverviewPage },
    { path:"/recipe/validation",label:"验证与发布",icon:SlidersHorizontal,element:ValidationPage },
  ] },
  {
    label: "设备与运行",
    icon: Cable,
    children: [
      { path:"/camera",label:"设备与采集",icon:Camera,element:CameraPage },
      { path: "/plc", label: "PLC 通讯", icon: Cable, element: PlcSettingsPage },
      { path:"/camera/calibration",label:"飞拍工位标定",icon:Camera,element:FlyshotCalibrationPage },
      { path: "/plc-logs", label: "通讯日志", icon: ScrollText, element: PlcLogsPage },
      { path: "/settings", label: "系统设置", icon: Settings, element:SettingsPage },
    ],
  },
];

export const navItems: NavItem[] = navEntries.flatMap((entry) => (isNavGroup(entry) ? entry.children : [entry]));
