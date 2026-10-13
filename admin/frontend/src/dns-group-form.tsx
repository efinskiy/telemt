import { useState } from 'react';
import type { Json, Node } from './api';
import { FieldsForm } from './components';
export function DNSGroupForm({ nodes, onSave }: { nodes: Node[]; onSave: (value: Json) => Promise<void> }) {
  const [members, setMembers] = useState<string[]>(nodes.map(node => node.id)), [active, setActive] = useState(nodes[0]?.id ?? '');
  return <><label className="label">Активный сервер<select aria-label="Активный сервер" value={active} onChange={event => { setActive(event.target.value); if (!members.includes(event.target.value)) setMembers([...members, event.target.value]); }}>{nodes.map(node => <option key={node.id} value={node.id}>{node.name}</option>)}</select></label><h3>Состав и приоритет резервов</h3><p>Порядок сверху вниз определяет приоритет. Активный сервер обязателен.</p><ol className="member-list">{members.map((id, index) => <li key={id}><span>{nodes.find(node => node.id === id)?.name ?? id}</span><div className="actions"><button type="button" className="quiet" disabled={index === 0} onClick={() => { const next = [...members]; [next[index-1],next[index]] = [next[index],next[index-1]]; setMembers(next); }}>Выше</button><button type="button" className="quiet" disabled={id === active} onClick={() => setMembers(members.filter(member => member !== id))}>Исключить</button></div></li>)}</ol>{nodes.filter(node => !members.includes(node.id)).map(node => <button className="quiet" key={node.id} type="button" onClick={() => setMembers([...members, node.id])}>Добавить {node.name}</button>)}<FieldsForm initial={{ name:'', domain:'', zone_id:'', record_a_id:'', record_aaaa_id:'', token:'' }} disabled={!nodes.length} fields={[
    {key:'name',label:'Название группы',required:true,description:'Название для кабинета и истории переключений.'},
    {key:'domain',label:'Домен прокси',required:true,description:'Общий домен совместимых нод. Записи должны уже указывать на активный сервер.'},
    {key:'zone_id',label:'Идентификатор зоны Cloudflare',required:true,description:'Zone ID зоны, которой принадлежат подключаемые записи.'},
    {key:'record_a_id',label:'Идентификатор записи A',description:'ID конкретной IPv4-записи. Можно оставить пустым, если используется только IPv6.'},
    {key:'record_aaaa_id',label:'Идентификатор записи AAAA',description:'ID конкретной IPv6-записи. Требуется хотя бы одна запись A или AAAA.'},
    {key:'token',label:'Токен Cloudflare',kind:'password',required:true,description:'Токен с доступом к DNS указанной зоны. Хранится зашифрованным.'},
  ]} onSave={async value => { if (!value.record_a_id && !value.record_aaaa_id) throw new Error('Укажите запись A или AAAA'); await onSave({ ...value, active_node: active, node_ids: members }); }} /></>;
}
