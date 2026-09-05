import {mkdirSync,copyFileSync,rmSync} from 'node:fs';
const root=new URL('../',import.meta.url);
if(process.argv.includes('--clean'))rmSync(new URL('dist/',root),{recursive:true,force:true});
for(const directory of ['src/wasm/','dist/wasm/']){
  const output=new URL(directory,root);mkdirSync(output,{recursive:true});
  for(const file of ['strk20_engine.js','strk20_engine.d.ts','strk20_engine_bg.wasm','strk20_engine_bg.wasm.d.ts']){
    copyFileSync(new URL(`../../crates/wasm/pkg/${file}`,root),new URL(file,output));
  }
}
