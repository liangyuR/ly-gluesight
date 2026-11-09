import type { ComponentType } from "react";
import { BookOpen, Cable, Camera, Crosshair, FileClock, FolderKanban, History, LayoutDashboard, ScanEye, ScrollText, Settings, type LucideIcon } from "lucide-react";
import InspectPage from "../pages/InspectPage";
import CameraPage from "../pages/CameraPage";
import FlyshotCalibrationPage from "../pages/FlyshotCalibrationPage";
import { PlcLogsPage, PlcSettingsPage } from "../pages/PlcPages";
import HistoryPage from "../pages/HistoryPage";
import SettingsPage from "../pages/SettingsPage";
import OperationGuidePage from "../pages/OperationGuidePage";
import ProjectsPage from "../pages/ProjectsPage";
import LibraryPage from "../features/workspace/LibraryPage";
import ProgressPage from "../features/workspace/ProgressPage";
import TeachingPiecePage from "../features/workspace/TeachingPiecePage";
import TeachingPage from "../features/workspace/TeachingPage";
import FlowDebugPage from "../features/workspace/FlowDebugPage";
import ValidationPage from "../features/workspace/ValidationPage";
import CalibrationPage from "../features/workspace/CalibrationPage";
import { AppLogPage } from "../features/logs";
import { inRecipe, SegmentsRedirect } from "../features/workspace/RecipeRoute";

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

export const projectsItem: NavItem = { path:"/projects",label:"项目",icon:FolderKanban,element:ProjectsPage };

export const settingsItem: NavItem = { path:"/settings",label:"系统设置",icon:Settings,element:SettingsPage };

export const navEntries: NavEntry[] = [
  { path:"/guide",label:"总览",icon:LayoutDashboard,element:OperationGuidePage },
  { label:"生产",children:[
    { path:"/inspect",label:"在线检测",icon:ScanEye,element:InspectPage },
    { path:"/history",label:"历史记录",icon:History,element:HistoryPage },
    { path:"/logs",label:"运行日志",icon:FileClock,element:AppLogPage },
  ] },
  { label:"工艺",children:[
    recipe,
  ] },
  { label:"设备",children:[
    { path:"/camera",label:"相机",icon:Camera,element:CameraPage },
    { path:"/camera/calibration",label:"标定",icon:Crosshair,element:FlyshotCalibrationPage },
    { path:"/plc",label:"PLC 通讯",icon:Cable,element:PlcSettingsPage },
    { path:"/plc-logs",label:"通讯日志",icon:ScrollText,element:PlcLogsPage },
  ] },
];

export const navItems: NavItem[] = navEntries.flatMap((entry) => (isNavGroup(entry) ? entry.children : [entry]));

/** 配方工作台各步骤的页面（段配方，StepIdV2），地址带配方编号：capture 走 /recipe/<编号>/capture，segments 与 corridor
 * 合在 …/teach（分段与示教，原 …/segments 转到这里），calibration 走 …/calibration，publish 走 …/validation。打开哪个配方只由地址决定。 */
const recipeSteps: PageRoute[] = [
  { path:"/recipe/:recipeId/progress",label:"进度总览",element:ProgressPage },
  { path:"/recipe/:recipeId/capture",label:"示教件",element:TeachingPiecePage },
  { path:"/recipe/:recipeId/teach",label:"分段与示教",element:TeachingPage },
  { path:"/recipe/:recipeId/flow",label:"流程调试",element:FlowDebugPage },
  { path:"/recipe/:recipeId/calibration",label:"试测",element:CalibrationPage },
  { path:"/recipe/:recipeId/validation",label:"发布",element:ValidationPage },
].map(step => ({ ...step, element:inRecipe(step.element), label:`${recipe.label} · ${step.label}` }))
  .concat({ path:"/recipe/:recipeId/segments",label:`${recipe.label} · 分段与示教`,element:SegmentsRedirect });

export const pageRoutes: PageRoute[] = [...navItems, ...recipeSteps, projectsItem, settingsItem];

export function matchRoute<T extends PageRoute>(routes: T[], pathname: string): T | undefined {
  return routes.filter(route => pathname === route.path || pathname.startsWith(route.path + "/")).sort((a, b) => b.path.length - a.path.length)[0];
}
