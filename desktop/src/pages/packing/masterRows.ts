import type { PackingItemDto } from "../../bindings/PackingItemDto";
import type { MobileRowDto } from "../../api";
const legacyIdentity=(r:Pick<PackingItemDto,"inbound"|"name"|"destination">)=>JSON.stringify([r.inbound.trim(),r.name.trim(),r.destination.trim()]);
export const masterIdentity=(r:Pick<PackingItemDto,"inbound"|"name"|"destination"|"seq">)=>JSON.stringify([r.inbound.trim(),r.name.trim(),r.destination.trim(),r.seq||0]);
export function applyMasterOverrides(rows:PackingItemDto[],raw?:string,waybillOnly=false,ledger=false):PackingItemDto[]{
  let map:Record<string,Partial<PackingItemDto>>={};try{map=JSON.parse(raw||"{}");}catch{return rows;}
  if(!map||typeof map!=="object"||Array.isArray(map))return rows;
  const counts=new Map<string,number>();for(const r of rows){const key=legacyIdentity(r);counts.set(key,(counts.get(key)||0)+1);}
  return rows.map(r=>{const value=map[masterIdentity(r)]||(counts.get(legacyIdentity(r))===1?map[legacyIdentity(r)]:undefined);if(!value||typeof value!=="object")return r;
    const allowed=["checked","waybill","customer","inbound","name","qty","qtyRaw","arrival","issues","destination","cargo","syncId","manualFields"];
    const patch=Object.fromEntries(Object.entries(value).filter(([k])=>(waybillOnly?["waybill",...(ledger?["checked"]:[]),"syncId","manualFields"]:allowed).includes(k)));
    if(waybillOnly&&r.waybill.trim())delete patch.waybill;
    if(waybillOnly&&Array.isArray(patch.manualFields))patch.manualFields=patch.manualFields.filter(f=>f==="waybill"||ledger&&f==="checked");
    // 合箱的计数归属以原表合并范围为准，旧手机快照不能把从属商品恢复成另一个箱数。
    if(r.quantityGroup){delete patch.qty;delete patch.qtyRaw;
      if(Array.isArray(patch.issues))patch.issues=[...new Set([...patch.issues.filter((c:string)=>c!=="qty_shared"),...r.issues.filter(c=>c==="qty_shared")])];
    }
    return {...r,...patch};});
}
export function fromMobileRow(row:MobileRowDto,source?:PackingItemDto):PackingItemDto {
 const result:PackingItemDto={sourceRow:source?.sourceRow??row.sourceRow,quantityGroup:source?.quantityGroup??row.quantityGroup,syncId:row.syncId,manualFields:row.manualFields,seq:row.seq||0,name:row.name,qty:row.qty,qtyRaw:row.qtyRaw||String(row.qty),cargo:row.cargo||{},inbound:row.inbound,destination:row.destination||"",arrival:row.arrival||"",customer:row.customer,waybill:row.waybill,checked:row.checked,issues:row.issues};
 if(source?.quantityGroup){result.qty=source.qty;result.qtyRaw=source.qtyRaw;
   result.issues=[...new Set([...result.issues.filter(c=>c!=="qty_shared"),...source.issues.filter(c=>c==="qty_shared")])];
 }
 return result;
}
