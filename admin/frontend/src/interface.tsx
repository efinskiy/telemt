import { useEffect, useState } from 'react';
import type { User } from './api';
import { stateLabel } from './labels';
import { Dialog } from './components';

const paths: Record<string, string> = {
  bell: 'M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9 M10 21h4',
  overview: 'M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z',
  server: 'M4 3h16v7H4z M4 14h16v7H4z M7 6.5h.01 M7 17.5h.01 M16 6.5h1 M16 17.5h1',
  profiles: 'M4 3h10l6 6v12H4z M14 3v6h6 M8 13h8 M8 17h5',
  certificate: 'M12 3a6 6 0 1 0 0 12 6 6 0 0 0 0-12 M8 14l-1 7 5-3 5 3-1-7',
  users: 'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2 M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8 M17 4a4 4 0 0 1 0 7 M22 21v-2a4 4 0 0 0-3-3.87',
  settings: 'M4 7h16 M4 17h16 M8 4v6 M16 14v6',
  shaping: 'M3 7h18 M6 12h12 M9 17h6 M12 17v4',
  activity: 'M2 12h4l3-8 6 16 3-8h4',
  globe: 'M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0 M3 12h18 M12 3c5 5 5 13 0 18-5-5-5-13 0-18',
  dns: 'M9 3h6v6H9z M3 16h6v5H3z M15 16h6v5h-6z M12 9v4 M6 16v-3h12v3',
  rotate: 'M20 7v5h-5 M4 17v-5h5 M6 7a7 7 0 0 1 12-2l2 3 M4 16l2 3a7 7 0 0 0 12-2',
  operations: 'M8 3h8v4H8z M8 5H5v16h14V5h-3 M8 12h8 M8 16h5',
  audit: 'M4 4h16v16H4z M8 8h8 M8 12h8 M8 16h4',
  shield: 'M12 3l8 3v6c0 5-8 9-8 9s-8-4-8-9V6z M9 12l2 2 4-4',
  menu: 'M4 6h16 M4 12h16 M4 18h16',
  collapse: 'M14 5l-7 7 7 7',
  expand: 'M9 5l7 7-7 7',
  sun: 'M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0 M12 2v2 M12 20v2 M2 12h2 M20 12h2 M5 5l1 1 M18 18l1 1 M5 19l1-1 M18 6l1-1',
  moon: 'M20 15a9 9 0 0 1-11-11 9 9 0 1 0 11 11',
  monitor: 'M3 3h18v13H3z M12 16v5 M8 21h8',
  logout: 'M9 4H4v16h5 M9 12h12 M17 8l4 4-4 4',
  more: 'M5 12h.01 M12 12h.01 M19 12h.01',
  arrow: 'M12 3v18 M6 9l6-6 6 6',
};
/** Consistent original line icons; no remote assets are required by the shell. */
export function Icon({ name, className = '' }: { name: string; className?: string }) {
  return <svg className={`icon ${className}`} width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.activity} /></svg>;
}
export function Brand() { return <div className="brand"><span className="brand-symbol" aria-hidden="true"><Icon name="shaping" /></span><div className="brand-name">telemt<span>Fleet</span></div></div>; }
const icons: Record<string, string> = { 'Обзор': 'overview', 'Серверы': 'server', 'Общие профили': 'profiles', 'Сертификаты': 'certificate', 'Пользователи': 'users', 'Конфигурация': 'settings', 'Шейпер': 'shaping', 'Диагностика': 'activity', 'Внешние проверки': 'globe', 'Переключение DNS': 'dns', 'Ротация IP': 'rotate', 'Операции': 'operations', 'Аудит': 'audit', 'Уведомления': 'bell', 'Администраторы': 'shield' };
export const pageIcon = (page: string) => icons[page] ?? 'overview';
const navigationGroups = [
  { title: 'Панель управления', pages: ['Обзор'] },
  { title: 'Инфраструктура', pages: ['Серверы', 'Общие профили', 'Сертификаты'] },
  { title: 'Трафик и доступ', pages: ['Пользователи', 'Конфигурация', 'Шейпер'] },
  { title: 'Наблюдение', pages: ['Диагностика', 'Внешние проверки'] },
  { title: 'Отказоустойчивость', pages: ['Переключение DNS', 'Ротация IP'] },
  { title: 'Система', pages: ['Операции', 'Аудит', 'Уведомления', 'Администраторы'] },
];
const descriptions: Record<string, string> = {
  'Обзор': 'Состояние инфраструктуры, нагрузка и подключения в одном месте.',
  'Серверы': 'Подключайте ноды и управляйте инфраструктурой кластера.',
  'Общие профили': 'Единые настройки и пользователи для группы серверов.',
  'Сертификаты': 'Общие SSL-сертификаты, сроки действия и продление.',
  'Пользователи': 'Доступ к прокси, подключения и индивидуальные ограничения.',
  'Конфигурация': 'Настройки выбранной ноды и управление их применением.',
  'Шейпер': 'Распределение канала, приоритеты и текущая нагрузка клиентов.',
  'Диагностика': 'Состояние telemt, маршрутов, upstream и WEB-сессий.',
  'Внешние проверки': 'Доступность прокси глазами независимых сетей.',
  'Переключение DNS': 'Переключение доменов между совместимыми серверами.',
  'Ротация IP': 'Пулы адресов нод и правила автоматической смены IP.',
  'Операции': 'Ход выполнения изменений и результаты по каждому серверу.',
  'Аудит': 'История действий администраторов и изменений в кабинете.',
  'Уведомления': 'Доставка событий ротации IP и смены сервера в Telegram.',
  'Администраторы': 'Учётные записи, роли и доступ к управлению Fleet.',
};
/** Page metadata keeps desktop and mobile navigation in the same hierarchy. */
export function pageContext(page: string) {
  return { group: navigationGroups.find(group => group.pages.includes(page))?.title ?? 'Fleet', description: descriptions[page] ?? '' };
}
type Theme = 'system' | 'light' | 'dark';
const stored = (key: string) => { try { return localStorage.getItem(key); } catch { return null; } };
// Private browsing may disallow preference persistence; in-memory controls still work.
const save = (key: string, value: string) => { try { localStorage.setItem(key, value); } catch {} };
export function useAppearance() {
  const [theme, setTheme] = useState<Theme>(() => { const value = stored('telemt.theme'); return value === 'light' || value === 'dark' ? value : 'system'; });
  const [collapsed, setCollapsed] = useState(() => stored('telemt.sidebar') === 'collapsed');
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => { document.documentElement.dataset.theme = theme === 'system' ? media.matches ? 'dark' : 'light' : theme; };
    apply(); save('telemt.theme', theme); media.addEventListener('change', apply);
    return () => media.removeEventListener('change', apply);
  }, [theme]);
  useEffect(() => { save('telemt.sidebar', collapsed ? 'collapsed' : 'expanded'); }, [collapsed]);
  return { theme, setTheme, collapsed, setCollapsed };
}
export function ThemeControl({ theme, change }: { theme: Theme; change: (theme: Theme) => void }) {
  return <label className="theme-control"><Icon name={theme === 'system' ? 'monitor' : theme === 'dark' ? 'moon' : 'sun'} /><select aria-label="Тема оформления" value={theme} onChange={event => change(event.target.value as Theme)}><option value="system">Системная</option><option value="light">Светлая</option><option value="dark">Тёмная</option></select></label>;
}
export function Navigation({ pages, page, select, user, collapsed, toggle, logout }: { pages: string[]; page: string; select: (page: string) => void; user: User; collapsed: boolean; toggle: () => void; logout: () => void }) {
  const [more, setMore] = useState(false);
  const visible = pages.filter(item => !['Администраторы', 'Уведомления'].includes(item) || user.role === 'owner');
  const quick = ['Обзор', 'Серверы', 'Пользователи', 'Шейпер'];
  const go = (item: string) => { select(item); setMore(false); };
  const item = (name: string) => <button key={name} title={name} aria-label={name} aria-current={page === name ? 'page' : undefined} className={page === name ? 'nav-active' : ''} onClick={() => go(name)}><Icon name={pageIcon(name)} /><span>{name}</span></button>;
  const grouped = () => navigationGroups.map(group => {
    const entries = group.pages.filter(name => visible.includes(name));
    return entries.length > 0 && <div className="nav-group" key={group.title}><div className="nav-group-title">{group.title}</div>{entries.map(item)}</div>;
  });
  return <><aside className="sidebar"><div className="sidebar-brand"><Brand /></div><nav className="desktop-nav" aria-label="Разделы Fleet">{grouped()}</nav><div className="sidebar-bottom"><button className="collapse-control" aria-label={collapsed ? 'Развернуть меню' : 'Свернуть меню'} title={collapsed ? 'Развернуть меню' : 'Свернуть меню'} onClick={toggle}><Icon name={collapsed ? 'expand' : 'collapse'} /><span>Свернуть меню</span></button><div className="account"><span className="avatar" title={`${user.username} · ${stateLabel(user.role)}`}>{user.username.slice(0, 1).toUpperCase()}</span><div><strong>{user.username}</strong><small>{stateLabel(user.role)}</small></div><button className="icon-button" title="Выйти" aria-label="Выйти" onClick={logout}><Icon name="logout" /></button></div></div></aside>
    <nav className="mobile-nav" aria-label="Разделы Fleet">{quick.map(item)}<button aria-label="Ещё разделы" aria-expanded={more} aria-haspopup="dialog" className={!quick.includes(page) ? 'nav-active' : ''} onClick={() => setMore(true)}><Icon name="more" /><span>Ещё</span></button></nav>
    {more && <Dialog title="Разделы Fleet" close={() => setMore(false)}><div className="more-navigation">{grouped()}</div><div className="mobile-account"><span>{user.username} · {stateLabel(user.role)}</span><button className="quiet" onClick={() => { setMore(false); logout(); }}>Выйти</button></div></Dialog>}
  </>;
}
