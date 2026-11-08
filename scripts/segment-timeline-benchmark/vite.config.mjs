import {defineConfig} from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import {fileURLToPath} from "node:url";
const here=path.dirname(fileURLToPath(import.meta.url)),repo=path.resolve(here,"../../..");
export default defineConfig({root:here,plugins:[{name:"benchmark-boundaries",enforce:"pre",resolveId(source,importer){
 if(!importer)return;
 const target=source.startsWith(".")?path.resolve(path.dirname(importer),source):source;
 const norm=target.replaceAll("\\","/");

 if(norm===repo.replaceAll("\\","/")+"/src/features/workspace/context"||norm===repo.replaceAll("\\","/")+"/src/features/workspace/context.tsx")return path.join(here,"context.tsx").replaceAll("\\","/");
 if(/(?:SegmentWorkbench|thumbs)\.tsx$/.test(importer)&&/\/src\/features\/workspace\/api(?:\.ts)?$/.test(norm))return path.join(here,"api.ts").replaceAll("\\","/");
 }},react()],resolve:{dedupe:["react","react-dom"]},build:{outDir:path.resolve(here,"../dist"),emptyOutDir:true},server:{host:"127.0.0.1",port:18778,strictPort:true,fs:{allow:[repo]}}});
