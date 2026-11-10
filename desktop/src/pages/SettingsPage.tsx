import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpenOutlined } from "@ant-design/icons";
import { api, errorText, SHOW_BOXPLAN_TAB_KEY, SHOW_LABEL_TAB_KEY, SHOW_PRODUCT_TAB_KEY } from "../api";

type Props={showLabelTab:boolean;onShowLabelTab:(v:boolean)=>void;showBoxplanTab:boolean;onShowBoxplanTab:(v:boolean)=>void;showProductTab:boolean;onShowProductTab:(v:boolean)=>void};
const OPTIONS=[
 {key:"packing_auto_open",label:"启动时打开上次表格",hint:"进入出货清单时恢复上次使用的本地表格"},
 {key:"kdocs_auto_sync",label:"自动检查云表格",hint:"使用动态心跳检查更新；手机和电脑仍可随时手动刷新"},
];
export default function SettingsPage(props:Props){
 const [settings,setSettings]=useState<Record<string,string>>({});
 const [configPath,setConfigPath]=useState("");
 const [error,setError]=useState("");
 const [busy,setBusy]=useState(false);
 const [saved,setSaved]=useState(false);
 const [hiddenTabsVisible,setHiddenTabsVisible]=useState(false);
 const reload=async()=>{
  setBusy(true);setError("");
  try{
   const [path,values]=await Promise.all([api.configFilePath().then(p=>{setConfigPath(p);return p;}),api.allSettings()]);
   setConfigPath(path);setSettings(values);
   props.onShowLabelTab(values[SHOW_LABEL_TAB_KEY]==="1");
   props.onShowBoxplanTab(values[SHOW_BOXPLAN_TAB_KEY]==="1");
   props.onShowProductTab(values[SHOW_PRODUCT_TAB_KEY]!=="0");
  }catch(e){setError(errorText(e));}finally{setBusy(false);}
 };
 useEffect(()=>{void reload();},[]);
 const save=async(key:string,on:boolean)=>{
  setBusy(true);setError("");setSaved(false);
  try{
   await api.setSetting(key,on?"1":"0");setSettings(prev=>({...prev,[key]:on?"1":"0"}));
   if(key===SHOW_LABEL_TAB_KEY)props.onShowLabelTab(on);
   if(key===SHOW_BOXPLAN_TAB_KEY)props.onShowBoxplanTab(on);
   if(key===SHOW_PRODUCT_TAB_KEY)props.onShowProductTab(on);
   setSaved(true);
  }catch(e){setError(errorText(e));}finally{setBusy(false);}
 };
 const row=(key:string,label:string,hint:string,checked:boolean)=><div className="setting-row" key={key}><div className="setting-main"><div className="setting-name">{label}</div><div className="setting-desc">{hint}</div></div><label className="switch"><input type="checkbox" checked={checked} disabled={busy||!!error} onChange={e=>void save(key,e.target.checked)}/><span className="switch-track" aria-hidden="true"/></label></div>;
 return <div className="page">
  <header className="page-head"><div><h1 className="page-title">设置</h1><p className="page-sub">偏好保存在本地 config.json，业务数据独立保存在数据库</p></div><div className="page-actions">{saved&&<span className="saved-hint">已写入配置文件 ✓</span>}<button className="btn" onClick={async()=>{try{await revealItemInDir(await api.logDir());}catch(e){setError(errorText(e));}}}><FolderOpenOutlined/>打开日志目录</button></div></header>
  <section className="card"><div className="card-head"><span className="card-title">配置文件</span></div><div className="card-body">
   <p className="settings-note">可以直接编辑配置文件。开关使用 true / false；手动修改后重启程序生效。下方开关会直接写入同一份文件。</p>
   <p className="config-file-path">{configPath||"正在定位配置文件…"}</p>
   <div className="page-actions"><button className="btn" disabled={!configPath} onClick={async()=>{try{await revealItemInDir(configPath);}catch(e){setError(errorText(e));}}}><FolderOpenOutlined/>定位配置文件</button><button className="btn" disabled={busy} onClick={()=>void reload()}>重新读取配置</button></div>
   {error&&<p role="alert" style={{color:"#b42318",whiteSpace:"pre-wrap"}}>{error}</p>}
  </div></section>
  <section className="card"><div className="card-head" onClick={e=>{if(e.detail===3)setHiddenTabsVisible(v=>!v);}}><span className="card-title">其它设置</span><span className="card-sub">修改后自动写入配置文件</span></div><div className="card-body"><div className="settings-list">
   {OPTIONS.map(o=>row(o.key,o.label,o.hint,settings[o.key]==="1"))}
   {hiddenTabsVisible&&<><div className="settings-group-title">导航栏显示</div>{row(SHOW_LABEL_TAB_KEY,"显示「箱唛生成」","打开后在导航栏显示",props.showLabelTab)}{row(SHOW_BOXPLAN_TAB_KEY,"显示「箱规计划」","打开后在导航栏显示",props.showBoxplanTab)}{row(SHOW_PRODUCT_TAB_KEY,"显示「产品标签」","打开后在导航栏显示",props.showProductTab)}</>}
  </div></div></section>
 </div>;
}
