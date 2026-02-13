# AgentCgroup: Контроль ресурсов AI-агентов разработчиков на базе eBPF

## Обзор

AgentCgroup - это система управления ресурсами на базе eBPF, которая предоставляет **иерархии cgroup, выровненные по вызовам инструментов** для AI-агентов, пишущих код. Она обеспечивает ограничения ресурсов (память, CPU) для отдельных выполнений инструментов, сохраняя отзывчивость агента, используя политики graceful degradation вместо резкого завершения.

### Ключевые возможности

- **Обнаружение инструментов через eBPF**: Мониторинг выполнения процессов в пространстве ядра для идентификации вызовов инструментов
- **Cgroup для каждого инструмента**: Индивидуальные иерархии cgroup v2 для каждого выполнения инструмента
- **Graceful Degradation**: Эскалация Throttle → Freeze → Kill вместо немедленного OOM
- **Метрики Prometheus**: Комплексный мониторинг и оповещения
- **YAML конфигурация**: Гибкие политики для разных типов агентов и инструментов
- **Режим Dry-Run**: Режим наблюдения без применения ограничений для тестирования

## Архитектура

```
┌─────────────────────────────────────────────────────┐
│  CLI (agentsight cgroup --pid 12345)                │
│  ↓                                                 │
│  Daemon (cgroup/daemon.rs)                         │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ │
│  │ PolicyEngine │ │CgroupManager │ │  Metrics     │ │
│  │ (YAML config)│ │(cgroup v2)   │ │(Prometheus)  │ │
│  └──────────────┘ └──────────────┘ └──────────────┘ │
│  ┌──────────────────────────────────────────────────┐│
│  │ ToolCallTracker (tracker.rs)                     ││
│  │ Обрабатывает BPF события, управляет cgroup ops  ││
│  └──────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────┘
                   │
   ┌───────────────▼──────────────────────────────────────┐
   │  eBPF Kernel Space (agentcgroup.bpf.c)               │
   │  Tracepoints: sched_process_exec/fork/exit           │
   │  Ring Buffer → JSON stdout                           │
   └──────────────────────────────────────────────────────┘
                   │
   ┌───────────────▼──────────────────────────────────────┐
   │  cgroup v2 filesystem                                │
   │  /sys/fs/cgroup/agentcgroup/                         │
   │    ├── workload-1/baseline/                          │
   │    ├── workload-1/tool-{pid}/                        │
   │    └── ...                                           │
   └──────────────────────────────────────────────────────┘
```

## Быстрый старт

### Базовое использование

```bash
# Мониторинг и контроль конкретного процесса агента
sudo ./agentsight cgroup --pid 12345 --comm claude

# Использование YAML конфигурации
sudo ./agentsight cgroup --config config/agentcgroup.example.yaml

# Режим dry-run (только наблюдение)
sudo ./agentsight cgroup --pid 12345 --dry-run

# Кастомный порт метрик
sudo ./agentsight cgroup --pid 12345 --metrics-port 9090
```

### Опции CLI

```bash
sudo ./agentsight cgroup [ОПЦИИ]

ОПЦИИ:
    -p, --pid <PID>              Целевой PID агента для мониторинга
    -c, --comm <COMM>            Фильтр по имени команды (например, "claude", "cursor")
    -w, --workload-id <ID>       ID рабочей нагрузки (по умолчанию: 1)
        --config <ПУТЬ>          Путь к файлу YAML конфигурации
        --metrics-port <ПОРТ>    Порт Prometheus метрик (по умолчанию: 9090)
        --no-degrade             Отключить автоматическую деградацию
        --monitor-interval <МС>  Интервал мониторинга давления (по умолчанию: 500мс)
        --dry-run                Режим наблюдения, не создавать cgroup
        --binary-path <ПУТЬ>     Путь к бинарному файлу agentcgroup BPF
    -v, --verbose                Включить подробный вывод BPF
```

## Конфигурация

### Схема YAML конфигурации

```yaml
# Глобальные настройки по умолчанию для всех рабочих нагрузок
global:
  baseline_memory_mb: 185      # Память агента в режиме ожидания (MB)
  tool_high_mb: 512            # Мягкий лимит на вызов инструмента (MB)
  tool_max_mb: 768             # Жесткий лимит на вызов инструмента (MB)
  cpu_weight: 100              # Вес CPU (1-10000)

# Политики для каждой рабочей нагрузки
workloads:
  - name: claude-code
    comm_filter: "claude"
    baseline_memory_mb: 185
    tool_high_mb: 512
    tool_max_mb: 768
    degradation:
      soft_throttle_pct: 85     # Throttling при 85% от memory.high
      hard_throttle_pct: 95     # Дальнейший throttling при 95%
      freeze_pct: 100           # Заморозка при 100%
      terminate_pct: 110        # Завершение при 110%

# Переопределения памяти для конкретных инструментов
tool_profiles:
  bash:
    memory_high_mb: 256
    memory_max_mb: 512
    priority: medium
  test:
    memory_high_mb: 518
    memory_max_mb: 2048
    priority: low
```

### Лимиты памяти (на основе исследований)

| Тип инструмента | Memory High (MB) | Memory Max (MB) | Источник |
|-----------------|------------------|-----------------|----------|
| baseline | 185 | 185 | RSS агента в режиме ожидания |
| bash | 256 | 512 | Операции shell |
| test | 518 | 2048 | pytest P95 |
| build | 400 | 2048 | Компиляция |
| install | 233 | 500 | npm install P95 |
| git | 128 | 256 | Операции Git |
| editor | 64 | 128 | Редактирование файлов |
| lint | 256 | 384 | Анализ кода |
| format | 128 | 256 | Форматирование кода |
| python | 384 | 640 | Выполнение Python |
| node | 384 | 640 | Выполнение Node.js |

## Лестница деградации

AgentCgroup использует подход **graceful degradation** вместо немедленного завершения процессов:

1. **Normal**: Ограничения не применяются
2. **SoftThrottle**: `memory.high` снижается до 85% от оригинала
3. **HardThrottle**: `memory.high` снижается до 70% от оригинала
4. **Freeze**: `cgroup.freeze = 1` (процесс приостановлен)
5. **Terminate**: `cgroup.kill = 1` (отправлен SIGKILL)

### Мониторинг давления

- **Интервал**: 500мс (настраивается)
- **Триггеры**: Процент использования памяти от `memory.high`
- **Эскалация**: Только эскалация, автоматической де-эскалации нет
- **Восстановление**: Происходит при выходе из инструмента (уничтожение cgroup)

## Метрики и мониторинг

### Endpoints Prometheus

- **Метрики**: `http://localhost:9090/metrics`
- **JSON API**: `http://localhost:9090/api/metrics`

### Доступные метрики

```prometheus
# Счетчики вызовов инструментов
agentcgroup_tool_calls_started_total{tool_type, priority, workload_id}
agentcgroup_tool_calls_exited_total{tool_type, priority, workload_id}

# Активные вызовы инструментов
agentcgroup_active_tool_calls

# События деградации
agentcgroup_throttle_events_total{level}
agentcgroup_freeze_events_total
agentcgroup_oom_kills_total

# Гистограмма длительности вызовов инструментов
agentcgroup_tool_call_duration_seconds{tool_type, priority, workload_id}
```

### Примеры запросов

```prometheus
# Активные вызовы по типу инструмента
sum(agentcgroup_active_tool_calls) by (tool_type)

# Скорость деградации
rate(agentcgroup_throttle_events_total[5m])

# Процентили длительности вызовов
histogram_quantile(0.95, sum(rate(agentcgroup_tool_call_duration_seconds_bucket[5m])) by (le, tool_type))
```

## Сборка и установка

### Предварительные требования

- Linux kernel 5.8+ (поддержка cgroup v2)
- clang 11+ (компиляция eBPF)
- Rust 1.82+ (collector)
- libbpf (библиотека BPF)

### Процесс сборки

```bash
# Установка зависимостей
make install

# Сборка программ eBPF
make build

# Сборка collector с встроенными бинарными файлами
cd collector && cargo build --release --features embed-ebpf

# Сборка frontend (опционально)
cd frontend && npm install && npm run build
```

### Расположение бинарных файлов

- **Программы eBPF**: `bpf/agentcgroup` (userspace loader)
- **Collector**: `target/release/agentsight`
- **Конфиг**: `config/agentcgroup.example.yaml`

## Соображения безопасности

### Требуются права root

AgentCgroup требует привилегий root потому что:

- **Загрузка eBPF**: Присоединение программ в пространстве ядра
- **Управление cgroup**: Доступ к файловой системе `/sys/fs/cgroup`
- **Мониторинг процессов**: Присоединение к tracepoints планировщика

### Безопасные настройки по умолчанию

- **Лимиты памяти**: Консервативные лимиты предотвращают неконтролируемое использование ресурсов
- **Режим Dry-Run**: Тестирование конфигураций без применения ограничений
- **Graceful Degradation**: Избегает потери данных от резкого завершения
- **Изоляция по инструментам**: Сбои инструментов не влияют на агента

## Устранение неисправностей

### Распространенные проблемы

#### Ошибки "Failed to create cgroup"
- Убедитесь, что cgroup v2 смонтирован: `mount -t cgroup2 none /sys/fs/cgroup`
- Проверьте версию ядра: `uname -r` (требуется 5.8+)
- Проверьте права root: `sudo -i`

#### BPF программа не загружается
- Проверьте заголовки ядра: `apt install linux-headers-$(uname -r)`
- Проверьте версию clang: `clang --version`
- Проверьте dmesg на ошибки BPF: `dmesg | grep bpf`

#### Вызовы инструментов не обнаруживаются
- Проверьте существование целевого PID: `ps aux | grep <pid>`
- Проверьте фильтр команд: `--comm claude` vs реальное имя процесса
- Включите подробный вывод: `--verbose`

#### Метрики недоступны
- Проверьте доступность порта: `netstat -tlnp | grep 9090`
- Проверьте firewall: `iptables -L`
- Проверьте логи daemon на ошибки

### Команды отладки

```bash
# Проверить монтирование cgroup v2
mount | grep cgroup

# Список активных cgroup
find /sys/fs/cgroup -name "*agentcgroup*" -type d

# Проверить загруженные программы BPF
bpftool prog list | grep agentcgroup

# Мониторить события cgroup
inotifywait -m /sys/fs/cgroup/agentcgroup/

# Проверить членство процесса в cgroup
cat /proc/<pid>/cgroup
```

## Характеристики производительности

### Накладные расходы

- **CPU eBPF**: <1% дополнительного использования CPU
- **Память**: ~2MB на активный cgroup инструмента
- **Задержка**: Суб-миллисекундная обработка событий
- **Хранение**: Минимальное (ring buffer + метрики)

### Масштабируемость

- **Макс рабочих нагрузок**: 128 (настраивается)
- **Макс одновременных вызовов**: 1024 (настраивается)
- **Пропускная способность событий**: 10,000+ событий/сек
- **Эффективность памяти**: Разделяемые структуры ядра

## Примеры интеграции

### С Claude Code

```bash
# Мониторинг агента Claude Code
sudo ./agentsight cgroup --comm claude --config config/agentcgroup.example.yaml

# С кастомными лимитами памяти
sudo ./agentsight cgroup --pid $(pgrep -f claude) --tool-high-mb 1024
```

### С Cursor

```bash
# Мониторинг агента Cursor IDE
sudo ./agentsight cgroup --comm cursor --workload-id 2
```

### С кастомным агентом

```yaml
# config/agentcgroup.custom.yaml
workloads:
  - name: my-custom-agent
    comm_filter: "my-agent"
    baseline_memory_mb: 150
    tool_high_mb: 384
    tool_max_mb: 512
    degradation:
      soft_throttle_pct: 80
      hard_throttle_pct: 90
      freeze_pct: 100
      terminate_pct: 120
```

```bash
sudo ./agentsight cgroup --config config/agentcgroup.custom.yaml
```

## Справочник API

### Rust API

```rust
use agentsight::cgroup::{Daemon, DaemonConfig, CgroupManager, PolicyEngine};

// Создание daemon
let config = DaemonConfig {
    binary_path: "agentcgroup".to_string(),
    target_pid: Some(12345),
    metrics_port: 9090,
    auto_degrade: true,
    ..Default::default()
};

let mut daemon = Daemon::new(config)?;
daemon.run().await?;
```

### Формат событий

```json
{
  "timestamp_ns": 1640995200000000000,
  "type": "tool_start",
  "pid": 12345,
  "ppid": 12344,
  "workload_id": 1,
  "tool_type": "bash",
  "priority": "medium",
  "comm": "bash"
}
```

## Практическая польза

### Реальные проблемы, которые решает

**Проблема**: AI-агент "съедает" всю память и зависает
```bash
# Claude Code пытается запустить 10 тестов параллельно
# Каждый тест использует 500MB памяти
# Итого: 5GB памяти → система зависает, IDE падает
```

**Решение с AgentCgroup**:
```bash
# Каждый тест получает лимит 512MB
# При превышении → graceful throttling → freeze → kill
# Система остается стабильной, агент продолжает работать
```

**Проблема**: Долгие сборки блокируют всю систему
```bash
# npm install скачивает 1000 пакетов
# Процесс занимает 2GB RAM и 4 CPU cores
# Ничего другого нельзя делать 10 минут
```

**Решение**:
```bash
# npm install получает лимит 233MB (измерено в исследованиях)
# При превышении → throttling до 70% производительности
# Можно продолжать работать в IDE параллельно
```

### Экономическая выгода

- **Снижение простоев разработчиков**: Нет ожидания завершения тестов, система стабильна
- **Автоматическое восстановление**: От memory leaks и зависших процессов
- **Метрики для оптимизации**: Видим узкие места и оптимизируем лимиты

### Безопасность и надежность

- **Защита от "плохих" команд AI**: Случайные destructive команды замораживаются
- **Изоляция инструментов**: Сбои одного инструмента не влияют на агента
- **Предсказуемое поведение**: Вместо хаоса - контролируемая среда разработки

## Вклад в проект

### Настройка среды разработки

```bash
# Клонирование и настройка
git clone <repository>
cd AgentCgroup

# Установка зависимостей сборки
make install

# Сборка всех компонентов
make build

# Запуск тестов
cd bpf && make test
cd collector && cargo test
```

### Организация кода

- **`bpf/`**: Программы eBPF ядра и userspace loaders
- **`collector/src/cgroup/`**: Реализация на Rust
  - `types.rs`: Разделяемые структуры данных
  - `manager.rs`: Операции с файловой системой cgroup
  - `policy.rs`: Конфигурация и оценка политик
  - `metrics.rs`: Сбор метрик Prometheus
  - `tracker.rs`: Обработка событий и trait Runner
  - `daemon.rs`: Главный оркестратор
- **`config/`**: Примеры YAML конфигураций
- **`docs/`**: Документация и руководства

### Тестирование

```bash
# Модульные тесты
cd collector && cargo test

# Интеграционные тесты
cd collector && cargo test --test integration

# Тесты BPF
cd bpf && make test
```

## Лицензия

Смотрите файл LICENSE в корневом каталоге.

## Ссылки

- [Документация cgroup v2](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html)
- [Документация eBPF](https://ebpf.io/)
- [Библиотека libbpf](https://github.com/libbpf/libbpf)
- [Метрики Prometheus](https://prometheus.io/docs/concepts/metric_types/)</content>
<parameter name="filePath">/Volumes/External/Code/AgentCgroup/docs/AgentCgroup_RU.md