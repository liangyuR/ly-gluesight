import {createContext,useContext} from "react";
export const BenchContext=createContext<any>(null);
export const useWorkspace=()=>useContext(BenchContext);
