//! GBLab controller: BLE GATT gamepad on the ESP32-H2-DEV-KIT-N4.
//!
//! Advertises PAD_SERVICE_UUID; the buttons characteristic notifies a 1-byte
//! bitmap whose bit order matches gb-core's joypad:
//! bit0 Right, bit1 Left, bit2 Up, bit3 Down, bit4 A, bit5 B, bit6 Select, bit7 Start.
//!
//! Buttons are active-low against the GPIO map in BUTTON_PINS (internal
//! pull-ups; wire each button between its GPIO and GND). The BOOT button
//! (GPIO9) doubles as A for bench testing without any wiring.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
use log::{info, warn};
use trouble_host::prelude::*;

esp_bootloader_esp_idf::esp_app_desc!();

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

/// 8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f001, little-endian for the AD payload.
const PAD_SERVICE_UUID_LE: [u8; 16] = [
    0x01, 0xf0, 0xb0, 0xa1, 0x33, 0x5c, 0x0e, 0x9d, 0x9a, 0x4c, 0x5b, 0x1e, 0x43, 0x2d, 0x7a,
    0x8f,
];

#[gatt_server]
struct Server {
    pad: PadService,
}

#[gatt_service(uuid = "8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f001")]
struct PadService {
    #[characteristic(uuid = "8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f002", read, notify, value = 0)]
    buttons: u8,
}

struct Pad<'d> {
    /// Right, Left, Up, Down, A, B, Select, Start.
    pins: [Input<'d>; 8],
    boot: Input<'d>,
}

impl Pad<'_> {
    fn read(&self) -> u8 {
        let mut bits = 0u8;
        for (i, pin) in self.pins.iter().enumerate() {
            if pin.is_low() {
                bits |= 1 << i;
            }
        }
        // BOOT acts as A for testing.
        if self.boot.is_low() {
            bits |= 1 << 4;
        }
        bits
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let software_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, software_interrupt.software_interrupt0);

    let pull_up = InputConfig::default().with_pull(Pull::Up);
    let pad = Pad {
        pins: [
            Input::new(peripherals.GPIO4, pull_up),  // Right
            Input::new(peripherals.GPIO5, pull_up),  // Left
            Input::new(peripherals.GPIO0, pull_up),  // Up
            Input::new(peripherals.GPIO1, pull_up),  // Down
            Input::new(peripherals.GPIO10, pull_up), // A
            Input::new(peripherals.GPIO11, pull_up), // B
            Input::new(peripherals.GPIO12, pull_up), // Select
            // GPIO13/14 are the 32K crystal pins on the DevKitM-1 layout.
            Input::new(peripherals.GPIO22, pull_up), // Start
        ],
        boot: Input::new(peripherals.GPIO9, pull_up),
    };

    let connector = BleConnector::new(peripherals.BT, Default::default()).unwrap();
    let controller: ExternalController<_, 20> = ExternalController::new(connector);

    let address = Address::random([0xff, 0x47, 0x42, 0x4c, 0x62, 0xff]);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host { mut peripheral, runner, .. } = stack.build();

    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "GBLab Pad",
        appearance: &appearance::human_interface_device::GAMEPAD,
    }))
    .unwrap();

    info!("GBLab Pad up, advertising");
    let _ = join(ble_task(runner), async {
        loop {
            match advertise(&mut peripheral, &server).await {
                Ok(conn) => {
                    info!("central connected");
                    select(gatt_events_task(&conn), pad_task(&server, &conn, &pad)).await;
                    info!("connection closed, advertising again");
                }
                Err(e) => {
                    warn!("advertise error: {e:?}");
                    Timer::after_secs(1).await;
                }
            }
        }
    })
    .await;
}

async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if let Err(e) = runner.run().await {
            panic!("ble runner error: {e:?}");
        }
    }
}

async fn gatt_events_task<P: PacketPool>(conn: &GattConnection<'_, '_, P>) {
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                info!("disconnected: {reason:?}");
                break;
            }
            GattConnectionEvent::Gatt { event } => {
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            _ => {}
        }
    }
}

/// Sample at 4 ms, report a stable 2-sample state on change.
async fn pad_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>, pad: &Pad<'_>) {
    let buttons = server.pad.buttons;
    let mut last_sample = pad.read();
    let mut reported = 0xFFu8;
    loop {
        let sample = pad.read();
        if sample == last_sample && sample != reported {
            reported = sample;
            if buttons.notify(conn, &sample).await.is_err() {
                warn!("notify failed, dropping connection loop");
                break;
            }
        }
        last_sample = sample;
        Timer::after_millis(4).await;
    }
}

async fn advertise<'values, 'server, C: Controller>(
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids128(&[PAD_SERVICE_UUID_LE]),
        ],
        &mut adv_data[..],
    )?;
    let mut scan_data = [0; 31];
    let scan_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(b"GBLab Pad")],
        &mut scan_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..adv_len],
                scan_data: &scan_data[..scan_len],
            },
        )
        .await?;
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    Ok(conn)
}
