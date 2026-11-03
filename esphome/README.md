# Варіант для Home Assistant (ESPHome)

[`power-monitor.yaml`](power-monitor.yaml) — прошивка для Wemos D1 mini, зібраної за варіантом **В** зі
[схеми](../hardware/README.md). У Home Assistant з'являться:

- **Світло** — «увімкнено», поки є 220 В;
- **Напруга акумулятора** — у вольтах, оновлюється раз на 30 с;
- **Сигнал Wi-Fi**.

## Прошивка

1. У Home Assistant встановіть доповнення **ESPHome** (Налаштування → Доповнення).
2. В ESPHome натисніть **New device**, дайте назву `power-monitor` і пропустіть вибір плати.
3. Відкрийте **Edit** у нового пристрою. ESPHome уже згенерував ключ шифрування: скопіюйте значення
   `key:` з розділу `api: encryption:`, воно знадобиться на кроці 5. Потім замініть вміст файлу на
   [`power-monitor.yaml`](power-monitor.yaml).
4. У файлі виправте `r1` і `r2` на номінали свого дільника (в омах). Найточніше — виміряти кожен резистор
   мультиметром до пайки.
5. Відкрийте **Secrets** (вгорі праворуч) і додайте:

   ```yaml
   wifi_ssid: "назва вашої мережі"
   wifi_password: "пароль від мережі"
   power_monitor_api_key: "ключ із кроку 3"
   power_monitor_ota_password: "будь-який пароль"
   ```

6. Підключіть D1 mini до комп'ютера через USB і натисніть **Install → Plug into this computer**
   (потрібен браузер Chrome або Edge). Наступні оновлення підуть по Wi-Fi.
7. Home Assistant сам знайде пристрій: Налаштування → Пристрої та служби → ESPHome → **Configure**.

Якщо пристрій не може підключитися до Wi-Fi, він відкриває точку доступу `power-monitor`, через яку
можна вказати іншу мережу.

## Перевірка напруги

Порівняйте **Напругу акумулятора** з мультиметром. Якщо різниця більша за ~1%, ще раз виміряйте R1 і R2 і
впишіть точні значення. Один крок АЦП D1 mini з R1 = 330k — це ~0.11 В, тож для акумулятора 12 В
краще поставити R1 = 56k (таблиця в [інструкції](../hardware/README.md#підбір-r1-під-свій-акумулятор)).

## Сповіщення

Пристрій лише передає дані, а повідомлення надсилає сам Home Assistant. Приклад автоматизації для
мобільного застосунку Home Assistant (замініть `binary_sensor.power_monitor_svitlo`,
`sensor.power_monitor_napruga_akumuliatora` і `notify.mobile_app_...` на свої назви з Налаштування →
Сутності):

```yaml
alias: Світло зникло або з'явилося
triggers:
  - trigger: state
    entity_id: binary_sensor.power_monitor_svitlo
    from: ["on", "off"]
    to: ["on", "off"]
actions:
  - action: notify.mobile_app_my_phone
    data:
      message: >-
        {{ '⚡ Світло з''явилося' if trigger.to_state.state == 'on' else '🔌 Світло зникло' }},
        акумулятор {{ states('sensor.power_monitor_napruga_akumuliatora') }} В
```
