const {test}=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),ts=require('typescript'),path=require('node:path');
const moduleValue={exports:{}};
vm.runInNewContext(ts.transpileModule(fs.readFileSync(path.join(__dirname,'../src/pages/packing/quantityGroups.ts'),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,{module:moduleValue,exports:moduleValue.exports});
const {quantityGroups,quantityText}=moduleValue.exports;
const row={sourceRow:374,quantityGroup:'I374:I375',qty:1,qtyRaw:'1',issues:[],name:'户外帽子-黑色'};
test('合箱提供可回原表核对的范围、行号和商品，箱数不重复',()=>{
 const second={...row,sourceRow:375,qty:0,issues:['qty_shared'],name:'22钉按摩梳，200/件'};
 const groups=quantityGroups([row,second,{...row,issues:['history_archive']}]);
 assert.equal(groups.length,1);assert.equal(groups[0].range,'I374:I375');assert.equal(groups[0].total,1);
 assert.equal(groups[0].items.length,2);assert.equal(groups[0].items[1].sourceRow,375);
 assert.equal(quantityText(second),'合箱（不另计箱数）');assert.equal(quantityText(row),'1 件');
});
