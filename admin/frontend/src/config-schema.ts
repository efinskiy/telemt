import { descriptions } from './config-descriptions';
import { type Json, object } from './api';
import { labelOf } from './labels';
export const managed = ['general', 'shaping', 'timeouts', 'censorship', 'upstreams', 'dc_overrides', 'server', 'web'];
/** Objects merge at the node; arrays replace atomically. Omitted fields must never overwrite peers. */
export function configPatch(before: Json, after: Json): Json | undefined {
  if (JSON.stringify(before) === JSON.stringify(after)) return undefined;
  if (Array.isArray(after) || after === null || typeof after !== 'object') return after;
  const old = object(before), patch: Record<string, Json> = {};
  for (const [key, value] of Object.entries(after)) { const delta = configPatch(old[key] ?? null, value); if (delta !== undefined) patch[key] = delta; }
  return Object.keys(patch).length ? patch : undefined;
}
export function editable(config: Record<string, Json>) { return Object.fromEntries(Object.entries(config).filter(([key]) => managed.includes(key)).map(([key, value]) => [key, key === 'server' ? { listeners: object(value).listeners ?? [] } : value])); }
export const templates: Record<string, Json> = {
  listeners: { ip: '', port: 443, transport: 'mtproxy', synlimit: false, reuse_allow: false, web_client_ip_source: 'x_forwarded_for', web_trusted_proxy_cidrs: [] },
  upstreams: { type: 'direct', weight: 1, enabled: true, scopes: '' },
  vhosts: { host: '', base_path: '', public_addr: '', decoy: { mode: 'static_directory', directory: '', index: 'index.html' }, profiles: [] }, profiles: { user: '', secret_mode: 'dd' },
};
export const choices: Record<string, string[]> = { transport: ['mtproxy', 'web'], secret_mode: ['plain', 'dd'], type: ['direct', 'socks4', 'socks5', 'shadowsocks'], log_level: ['debug', 'verbose', 'normal', 'silent'], web_client_ip_source: ['x_forwarded_for'], resolve: ['never', 'startup'], me_level: ['normal','silent','debug'], me_socks_kdf_policy: ['strict','compat'], me_bind_stale_mode: ['never','ttl','always'], rst_on_close: ['off','errors','always'], me_floor_mode: ['static','adaptive'], me_route_no_writer_mode: ['async_recovery_failfast','inline_recovery_legacy','hybrid_async_persistent'], me_writer_pick_mode: ['sorted_rr','p2c'], unknown_sni_action: ['drop','mask','accept','reject_handshake'], carrier: ['https','https-lanes','websocket','websocket-lanes'], carrier_method: ['post','put'], carrier_negotiation_aggressiveness: ['conservative','balanced','aggressive'], decoy_fasttrack_mode: ['off','shadow','enforce'], http_connection_capacity_action: ['drop','wait','respond'], body_capture: ['off','metadata','prefix','full'], mode: ['http_upstream', 'static_directory'] };
export const optionals: Record<string, Record<string, Json>> = {
  general: { data_path: '', proxy_secret_path: '', proxy_secret_url: '', proxy_config_v4_cache_path: '', proxy_config_v4_url: '', proxy_config_v6_cache_path: '', proxy_config_v6_url: '', ad_tag: '', middle_proxy_nat_ip: '', middle_proxy_nat_stun: '', unknown_dc_log_path: '', update_every: 300 },
  links: { public_host: '', public_port: 443 },
  censorship: { mask_host: '', mask_unix_sock: '' },
  listeners: { port: 443, client_mss: '1400', announce: '', announce_ip: '', proxy_protocol: false },
  profiles: { max_sessions: 1, max_streams: 1, max_streams_per_session: 1 },
  upstreams: { interface: '', bind_addresses: [], bindtodevice: '', user_id: '', username: '', password: '', ipv4: true, ipv6: true, prefer: 4 },
};
const hints: Record<string, string> = {
  general:'Режим маршрутизации и параметры процесса. Изменения применяются после проверки нодой; часть параметров требует перезапуска процесса.',
  shaping:'Локальный контроль скорости по IP. Все соединения одного IP делят бюджет; неиспользованная ёмкость доступна объёмным передачам.',
  timeouts:'Длительность ожидания сетевых операций. Слишком короткие интервалы обрывают подключения на медленных сетях.',
  censorship:'Внешний вид FakeTLS и адрес обычного HTTPS-сайта, на который попадают неподтверждённые подключения.',
  upstreams:'Исходящие маршруты до Telegram. Порядок и веса влияют на выбор маршрута; список сохраняется целиком.',
  dc_overrides:'Явные адреса дата-центров Telegram. Ключ — номер DC, значения — адреса маршрутизации.',
  web:'WEB-транспорт, домены и доступ пользователей. HTTPS завершается на внешнем обратном прокси.',
  listeners:'Локальные IP, порты и транспорт принимаемых подключений. Новые адреса должны быть назначены интерфейсам сервера.',
  up_bps:'Общая доступная скорость от клиентов, в бит/с. Например, 180000000 для бюджета 180 Мбит/с.',
  down_bps:'Общая доступная скорость к клиентам, в бит/с. Оставьте запас относительно измеренной пропускной способности.',
  bulk_up_bps:'Потолок объёмных отправок при конкуренции с короткими потоками. Ноль — автоматический расчёт. При свободном канале разрешено заимствование.',
  bulk_down_bps:'Потолок объёмных загрузок при конкуренции. Ноль — автоматический расчёт. Свободная ёмкость доступна загрузкам.',
  bulk_idle_up_bps:'Максимум объёмной отправки без конкуренции. Ноль — весь канал; значение должно быть не ниже бюджета при конкуренции и не выше канала.',
  bulk_idle_down_bps:'Максимум объёмной загрузки без конкуренции. Позволяет оставить запас даже при отсутствии коротких потоков.',
  sustained_rate_bps:'Минимальная скорость IP для длительной нагрузки. Суммируются все пользователи и соединения IP, направления оцениваются отдельно.',
  sustained_secs:'Сколько секунд подряд IP должен превышать порог скорости. Дополнительно требуется минутный объём. Ноль отключает классификацию длительной нагрузки IP.',
  sustained_min_bytes:'Минимальный трафик IP за последние 60 секунд для признания нагрузки длительной. Один всплеск без требуемой длительности не делает IP тяжёлым.',
  sustained_recovery_secs:'Сколько секунд скорость должна быть ниже половины порога без ожидания бюджета. Переподключение не сбрасывает состояние IP.',
  main_dc_priority:'Положительный DC 1–5 после аутентификации даёт подсказку приоритета. Это не распознавание сообщений. Длительная нагрузка IP и все лимиты имеют преимущество; требуется ненулевая длительность нагрузки.',
  priority_percent:'Доля канала, гарантируемая коротким потокам при наличии спроса. По умолчанию 25%; неиспользованный резерв занимают загрузки.',
  bulk_enter_bytes:'После такого объёма за последние 60 секунд поток переходит в объёмный класс. Направления считаются отдельно.',
  bulk_exit_bytes:'Возврат в короткий класс при снижении минутного объёма ниже порога. Должен быть меньше порога входа.',
  max_ips:'Максимум записей IP в памяти. При заполнении новые IP отклоняются, существующие продолжают обслуживаться.',
  use_middle_proxy:'Включает маршрутизацию через Middle Proxy Telegram вместо прямого соединения с DC.',
  tls:'Разрешает FakeTLS-ссылки ee для MTProxy. Это не TLS-терминация WEB: сертификат WEB обслуживается обратным прокси.',
  secure:'Разрешает MTProxy с padding и ключами dd. Этот режим также нужен соответствующим WEB-профилям.',
  classic:'Разрешает обычные ключи MTProxy без префикса.',
  tls_domain:'Домен SNI для FakeTLS-маскировки и ссылок подключения. Должен соответствовать выбранной схеме маскировки.',
  mask_host:'Адрес настоящего HTTPS-сайта для неподтверждённых подключений. Не направляйте его обратно на этот же слушатель.',
  mask_port:'TCP-порт обычного HTTPS-сайта, обычно 443.', mask:'Перенаправляет неподтверждённые подключения на обычный сайт.',
  host:'Публичный домен WEB в нижнем регистре (для IDN — ACE/punycode).',
  public_addr:'Публичная пара IP:порт для внутренней маршрутизации и KDF. IPv6 указывается в квадратных скобках.',
  base_path:'Префикс WEB без начального и конечного слеша. Должен совпадать с маршрутизацией обратного прокси.',
  profiles:'Пользователи из раздела доступа, которым разрешено подключаться через этот WEB-домен.',
  user:'Имя существующего пользователя прокси. Создайте ключ в разделе «Пользователи».',
  secret_mode:'Формат существующего ключа пользователя: plain без префикса либо dd с padding.',
  decoy:'Обычный сайт для браузеров и неподтверждённых запросов.', mode:'Источник обычного сайта: HTTP-сервер либо локальный каталог.',
  upstream:'HTTP-адрес обычного сайта для этого WEB-домена.', directory:'Каталог обычного сайта внутри файловой системы ноды.', index:'Имя индексного файла обычного сайта.',
  resolve:'never — без предварительного DNS-разрешения; startup — закрепить результат при загрузке конфигурации.',
  web_client_ip_source:'Заголовок с исходным IP клиента от доверенного обратного прокси.',
  web_trusted_proxy_cidrs:'Только эти сети могут передавать исходный IP. Укажите адрес HTTPS-прокси, не открывайте доверие всему интернету.',
  proxy_protocol:'Принимает PROXY protocol перед MTProxy. Включайте только при согласованной настройке доверенного балансировщика.',
  port:'TCP-порт слушателя на ноде. Должен быть свободен и доступен согласно правилам сети.',
  ip:'Локальный адрес слушателя. Для WEB за Nginx обычно используется loopback.',
  type:'Тип исходящего маршрута. Смена типа заменяет только параметры этого элемента списка.',
  address:'Адрес и порт исходящего SOCKS-прокси.', url:'URL маршрута. Может содержать учётные данные; храните как секрет.',
  interface:'Локальный IP исходящего соединения.', bind_addresses:'Пул локальных исходящих IP.', bindtodevice:'Имя Linux-интерфейса для SO_BINDTODEVICE.',
  weight:'Относительный вес при выборе исходящего маршрута.', scopes:'Области применения исходящего маршрута в формате ноды.',
  enabled:'Включает или отключает этот компонент.', log_level:'Подробность журнала. Подробный и отладочный режимы увеличивают объём записей.',
  carriers:'Упорядоченный список WEB-транспортов. Выключение отключает автовыбор; остаётся транспорт web.carrier; элементы нельзя дублировать.',
  show:'Все пользователи (*) либо явный список имён. Пустой список скрывает все ссылки.',
  synlimit:'Системный механизм ограничения SYN. Требует доступного firewall и прав на ноде.',
};
export function helpFor(path: string) {
  const key = path.split('.').at(-1)!;
  const restart = path.startsWith('web.limits.') ? ' Изменение требует перезапуска процесса.' : '';
  if (descriptions[key]) return descriptions[key][1] + restart;
  if (hints[key]) return hints[key] + restart;
  const unit = key.endsWith('_ms') ? 'Значение в миллисекундах.' : key.endsWith('_secs') || key.endsWith('_seconds') ? 'Значение в секундах.' : key.endsWith('_bytes') ? 'Размер в байтах.' : key.endsWith('_bps') ? 'Скорость в бит/с.' : key.endsWith('_pct') ? 'Доля в процентах.' : '';
  return `${labelOf(key)} в разделе «${labelOf(path.split('.')[0])}». ${unit} Значение проверяется нодой перед сохранением.`;
}
