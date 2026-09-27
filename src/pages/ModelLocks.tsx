import { useEffect, useState } from 'react';
import { usePoolStatus } from '../components/dashboard/PoolControl';
const key = (model: string) => model.startsWith('claude') ? 'claude' : model;
const duration = (seconds: number) => { const n = Math.max(0, seconds); return `${Math.floor(n/86400)}天 ${Math.floor(n%86400/3600)}时 ${Math.floor(n%3600/60)}分 ${n%60}秒`; };
export default function ModelLocks() {
    const [page,setPage] = useState(0); const {status,error} = usePoolStatus(page);
    const [now,setNow] = useState(Math.floor(Date.now()/1000)); const [offset,setOffset] = useState(0);
    useEffect(() => { if(status) setOffset(status.server_time - Math.floor(Date.now()/1000)); }, [status]);
    useEffect(() => { const timer=setInterval(()=>setNow(Math.floor(Date.now()/1000)),1000); return ()=>clearInterval(timer); },[]);
    return <div className="h-full overflow-auto p-6 space-y-4">
        <div><h1 className="text-2xl font-bold">模型冷却</h1><p className="text-sm opacity-60">{status?.locked_account_count ?? 0} 个账号 · {status?.locked_model_count ?? 0} 个模型锁。展示挂锁账号的全部模型额度，倒计时到期后由调度器重新判断可用状态。</p></div>
        {error && <p role="alert" className="text-error">{error}</p>}
        {status?.locked_accounts.length === 0 && <p className="p-8 text-center opacity-60">当前没有挂锁账号</p>}
        {status?.locked_accounts.map(account => <section key={account.account_id} className="rounded-xl border border-gray-200 dark:border-base-300 bg-white dark:bg-base-100 p-4">
            <h2 className="font-semibold break-all">{account.email || account.account_id} <span className="text-xs opacity-60">{account.loaded ? '已加载' : '未加载 / 已禁用'}</span></h2>
            <div className="overflow-x-auto"><table className="table table-sm"><thead><tr><th>模型</th><th>额度</th><th>状态</th><th>剩余时间</th><th>到期时间</th><th>来源 / 次数</th></tr></thead><tbody>
                {[...new Set([...Object.keys(account.quotas ?? {}), ...Object.keys(account.locks)])].sort().map(model => {
                    const lock=account.locks[key(model)]; const left=lock ? Math.max(0,lock.until-now-offset) : 0;
                    return <tr key={model}><td className="font-mono text-xs">{model}</td><td>{account.quotas?.[model] === undefined ? '无额度数据' : `${account.quotas[model]}%`}</td><td className={left ? 'text-orange-600' : 'text-green-600'}>{left ? (lock.lock_type === 'exact' ? '精确锁' : '短时锁') : '无活动锁'}</td><td className="tabular-nums">{left ? duration(left) : '—'}</td><td>{left ? new Date(lock.until*1000).toLocaleString() : '—'}</td><td title={lock?.message}>{left ? `${lock.status} / ${lock.transient_count || '精确期限'}` : '—'}</td></tr>;
                })}
            </tbody></table></div>
        </section>)}
        <div className="flex items-center gap-4"><button className="btn btn-sm" disabled={page===0} onClick={()=>setPage(p=>p-1)}>上一页</button><span>第 {page+1} 页，每页 100 个账号</span><button className="btn btn-sm" disabled={!status || (page+1)*100>=status.locked_account_count} onClick={()=>setPage(p=>p+1)}>下一页</button></div>
    </div>;
}
