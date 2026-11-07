// Compare only synthetic fixtures against an explicitly supplied original checkout.
// Usage: node scripts/differential.mjs /path/to/virtualdj-connect /path/to/esbuild
import {mkdtemp, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve, dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
import {spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
const [source, esbuild] = process.argv.slice(2);
if (!source || !esbuild) throw new Error('Pass original checkout and esbuild executable paths');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temp = await mkdtemp(join(tmpdir(), 'virtualdj-rs-differential-'));
process.env.TZ='UTC';
const run = (command,args,options={}) => {
  const result=spawnSync(command,args,{encoding:'utf8',...options});
  if(result.error || result.status!==0) throw new Error(result.stderr || String(result.error));
  return result.stdout;
};
try {
 const bundle=join(temp,'original.cjs');
 run(resolve(esbuild),[join(resolve(source),'src/index.ts'),'--bundle','--platform=node','--format=cjs',`--outfile=${bundle}`]);
 const original=createRequire(import.meta.url)(bundle);
 run('cargo',['build','--locked','--example','differential'],{cwd:root});
 const binary=join(root,'target/debug/examples',process.platform==='win32'?'differential.exe':'differential');
 const now='2026-11-07T12:00:00.000Z';
 const OriginalDate=Date;
 globalThis.Date=class extends OriginalDate {constructor(...args){super(...(args.length?args:[now]));}static now(){return new OriginalDate(now).getTime();}};
 const entries=[
  '#EXTVDJ:<artist>A</artist><title>T</title>\n/path.mp3\n',
  '#EXTM3U\r\n#EXTVDJ:<time>14:30</time><lastplaytime>1704196200</lastplaytime><artist>Årtist</artist><title>Song = Mix</title><remix>Extended</remix>\r\nnetsearch://bp123\r\n',
  '#EXTVDJ:<title>Only Title</title>\nnetsearch://other\n',
  '#EXTVDJ:<lastplaytime>42suffix</lastplaytime>\n/x\n#EXTVDJ:<artist>Last</artist>\n/last\n',
  '#EXTVDJ:<title>No path</title>',
 ];
 const inputs=[],expected=[];
 for(const content of entries){await writeFile(join(temp,'fixture.m3u'),content);const parser=new original.VirtualDjM3uParser({basePath:temp});parser.historyFile=join(temp,'fixture.m3u');expected.push(JSON.parse(JSON.stringify(parser.parseM3uFile())));inputs.push({op:'history',now,content});}
 const deck=(number,title,audible,path='/Music/song.mp3')=>Object.fromEntries(Object.entries({loaded:'yes',is_audible:audible?'yes':'no',"get_loaded_song 'title'":title,"get_loaded_song 'artist'":'Artist',"get_loaded_song 'album'":'Album',"get_loaded_song 'genre'":'House',"get_loaded_song 'key'":'Am','get_bpm absolute':'128.5','get_time total':'4:03',get_filepath:path}).map(([verb,value])=>[`deck ${number} ${verb}`,value]));
 const first={sandbox:'no',...deck(1,'Song One',false),...deck(2,'Song Two',true)};
 const steps=[first,{...first,...deck(1,'Song One',true)},{...first,sandbox:'yes'},{...first,sandbox:'yes'},{...first,sandbox:'no'},{...first,...deck(1,'Song One',true),...deck(2,'Song Two',false)}, {...first,'deck 1 loaded':'no','deck 2 loaded':'no'}];
 const control=new original.VirtualDjNetworkControl({decks:[1,2],fetchFn:async(url)=>{const script=new URL(url).searchParams.get('script');const value=current[script]??'';if(value&&typeof value==='object')return {ok:false,status:500,text:async()=>''};return {ok:true,status:200,text:async()=>value};}});
 let current={},events=[];
 control.on('track',track=>events.push({type:'track',track}));control.on('onair',(active,deck)=>events.push({type:'onAir',active,deck}));control.on('sandbox',active=>events.push({type:'sandbox',active}));control.isRunning=true;
 const networkExpected=[];
 for(const step of steps){current=step;events=[];await control.poll();networkExpected.push({events:JSON.parse(JSON.stringify(events)),sandboxed:control.sandboxed,onAir:control.onAir,onAirDeck:control.onAirDeck});}
 control.stop();expected.push(networkExpected);inputs.push({op:'network',decks:[1,2],steps});
 const output=run(binary,[],{input:inputs.map(input=>JSON.stringify(input)).join('\n')+'\n',env:{...process.env,TZ:'UTC'}}).trim().split('\n').map(line=>JSON.parse(line));
 assert.deepEqual(output,expected);
 console.log(JSON.stringify({passed:true,historyFixtures:entries.length,networkPollTransitions:steps.length,sourceRevision:'3850cca1d690afa0dc19f2d3e13d89f94f1677d2'}));
} finally {await rm(temp,{recursive:true,force:true});}
