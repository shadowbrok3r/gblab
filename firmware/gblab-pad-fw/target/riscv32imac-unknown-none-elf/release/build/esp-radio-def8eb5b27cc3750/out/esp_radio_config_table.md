
| Option | Stability | Default&nbsp;value | Allowed&nbsp;values |
|--------|:---------:|:------------------:|:-------------------:|
| <p>**ESP_RADIO_CONFIG_WIFI_MAX_BURST_SIZE**</p> <p>See [embassy-net's documentation](https://docs.rs/embassy-net-driver/0.2.0/embassy_net_driver/struct.Capabilities.html#structfield.max_burst_size)</p> | ⚠️ Unstable | 3 | Positive integer or 0
| <p>**ESP_RADIO_CONFIG_WIFI_MTU**</p> <p>MTU, see [embassy-net's documentation](https://docs.rs/embassy-net-driver/0.2.0/embassy_net_driver/struct.Capabilities.html#structfield.max_transmission_unit)</p> | ⚠️ Unstable | 1492 | Positive integer
| <p>**ESP_RADIO_CONFIG_DUMP_PACKETS**</p> <p>Dump packets via an info log statement</p> | ⚠️ Unstable | false | 
| <p>**ESP_RADIO_CONFIG_EVENT_CHANNEL_CAPACITY**</p> <p>Capacity of the internal event channel</p> | ⚠️ Unstable | 2 | Positive integer or 0
| <p>**ESP_RADIO_CONFIG_EVENT_CHANNEL_SUBSCRIBERS**</p> <p>Max subscriber count of the internal event channel</p> | ⚠️ Unstable | 2 | Positive integer or 0
