const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),ts=require('typescript');
const state={exports:{}};
const source=fs.readFileSync(path.join(__dirname,'../src/pages/packing/specialShipments.ts'),'utf8');
vm.runInNewContext(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,{module:state,exports:state.exports});
const {specialLabel,specialIssue,hasSpecialShipment,suggestedSpecialLabel}=state.exports;
const row={customer:'九宸',name:'眼镜',destination:'休斯顿',arrival:'发深圳',issues:[]};
test('发深圳和亚马逊标签独立，普通业务文字不获得免单号权限',()=>{
 assert.equal(suggestedSpecialLabel(row),'发深圳');
 assert.equal(hasSpecialShipment(row),false);
 assert.equal(specialLabel({...row,issues:[specialIssue('发深圳')]}),'发深圳');
 assert.equal(specialLabel({...row,issues:[specialIssue('亚马逊')]}),'亚马逊');
 assert.equal(suggestedSpecialLabel({...row,arrival:'',customer:'果粒橙'}),'亚马逊');
 assert.equal(suggestedSpecialLabel({...row,arrival:'',customer:'深圳部'}),undefined);
});
