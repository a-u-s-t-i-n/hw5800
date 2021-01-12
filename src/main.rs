use std::process;

extern crate paho_mqtt as mqtt;

extern crate clap;

pub mod devices;
pub mod hw5800;

/// Open a rtl-sdr device and watch for HW5800 messages, calling
/// the provided function when one is seen.
fn hw5800<F: Fn(&hw5800::HW5800Status) -> ()>(f: F, device: u32) {
    let (mut ctl, mut reader) = rtlsdr_mt::open(device)
        .expect(&format!("Could not open RTL-SDR device {}", device));

    ctl.enable_agc().expect("Could not set auto-gain");
    ctl.set_ppm(60).expect("Could not set PPM");
    ctl.set_center_freq(345_000_000)
        .expect("Could not set frequency");
    ctl.set_sample_rate(1_000_000)
        .expect("Could not set sample rate");

    let mut hw5800 = hw5800::HW5800::new(f);

    reader
        .read_async(4, 32768, move |bytes| {
            (0..bytes.len()).step_by(2).for_each(|i| {
                let real: f32 = (bytes[i] as f32) - 127.;
                let imag: f32 = (bytes[i + 1] as f32) - 127.;
                hw5800.add_sample(real, imag);
            });
        })
        .expect("Error reading from RTL-SDR");
}

fn main() {
    let args = clap::App::new("hw5800")
        .version("0.0.0")
        .set_term_width(80)
        .about(
            "Use a RTL-SDR receiver to receive and parse Honeywell 5800-type \
            radio transmissions, sending the results to a MQTT server.",
        )
        .arg(clap::Arg::with_name("server")
            .short("s")
            .long("server")
            .value_name("MQTT_SERVER")
            .takes_value(true)
            .help("MQTT server. If not provided MQTT messages will not be sent."))
        .arg(clap::Arg::with_name("port")
            .short("p")
            .long("port")
            .value_name("PORT")
            .takes_value(true)
            .help("MQTT server port. Defaults to 1883."))
        .arg(clap::Arg::with_name("user")
            .short("u")
            .long("user")
            .value_name("USER")
            .takes_value(true)
            .help("MQTT user name."))
        .arg(clap::Arg::with_name("password")
            .short("P")
            .long("password")
            .value_name("PASSWORD")
            .takes_value(true)
            .help("MQTT password. Ignore if no user provided"))
        .arg(clap::Arg::with_name("device-file")
            .short("d")
            .long("device-file")
            .value_name("FILE")
            .takes_value(true)
            .help("File containing device identifications.")
            .long_help("File containing device identifications. \
            Each line contains a 3-byte hex device ID and a device type. \
            Valid device types: {door, motion}"))
        .arg(clap::Arg::with_name("rtl-number")
            .short("r")
            .long("rtl-number")
            .value_name("NUMBER")
            .takes_value(true)
            .help("The RTL device number to use."))
        .arg(clap::Arg::with_name("client-id")
            .short("i")
            .long("client-id")
            .value_name("ID")
            .takes_value(true)
            .help("The client ID to use when connecting to MQTT."))
        .arg(clap::Arg::with_name("key-store")
            .short("k")
            .long("key-store")
            .value_name("KEY_STORE")
            .takes_value(true)
            .help("File containing the SSL key store to use (.pem file)"))
        .arg(clap::Arg::with_name("trust-store")
            .short("t")
            .long("trust-store")
            .value_name("TRUST_STORE")
            .takes_value(true)
            .help("File containing the SSL trust store to use (.crt file)"))
        .get_matches();

    // parse the device number.
    let rtl_num = if let Some(rn) = args.value_of("rtl-number") {
        if let Ok(rnu) = rn.parse::<u32>() {
            rnu
        } else {
            println!("Count not parse RTL device number from: {}", rn);
            return;
        }
    } else {
        0
    };

    // parse the device file
    let devs = if let Some(devfile) = args.value_of("device-file") {
        let file =
            std::fs::File::open(devfile).expect("Error opening device file");
        let reader = std::io::BufReader::new(file);
        devices::DeviceStore::load(reader).expect("Error parsing device file")
    } else {
        devices::DeviceStore::new()
    };

    // if the server is provided, include MQTT posting
    // code in the callback.
    if let Some(server) = args.value_of("server") {
        let port = args.value_of("port").unwrap_or("1883");
        let mut create_opts = mqtt::CreateOptionsBuilder::new();
        create_opts =
            create_opts.server_uri(format!("tcp://{}:{}", server, port));

        if let Some(client_id) = args.value_of("client-id") {
            create_opts = create_opts.client_id(client_id);
        }
        // create_opts done.

        let mut conn_opts = mqtt::ConnectOptionsBuilder::new();

        if let Some(user) = args.value_of("user") {
            conn_opts.user_name(user);
            if let Some(password) = args.value_of("password") {
                conn_opts.password(password);
            }
        }

        let mut ssl_opts = mqtt::SslOptionsBuilder::new();
        //ssl_opts.ssl_version(mqtt::ssl_options::SslVersion::Tls_1_2);
        let mut ssl_opts_set = false;
        if let Some(keystore) = args.value_of("key-store") {
            ssl_opts
                .key_store(keystore)
                .expect("Error loading SSL key store");
            ssl_opts_set = true;
        }

        if let Some(truststore) = args.value_of("trust-store") {
            ssl_opts
                .trust_store(truststore)
                .expect("Error loading SSL trust store");
            ssl_opts_set = true;
        }

        if ssl_opts_set {
            conn_opts.ssl_options(ssl_opts.finalize());
        }

        let cli = mqtt::Client::new(create_opts.finalize())
            .expect("Could not create MQTT instance");

        // Connect and wait for it to complete or fail
        if let Err(e) = cli.connect(conn_opts.finalize()) {
            println!("Unable to connect to MQTT: {:?}", e);
            process::exit(1);
        }

        hw5800(
            |status: &hw5800::HW5800Status| {
                let payload = devs.as_json(status);
                println!(
                    "PUBLISHING: Device: {:02X} status: {}",
                    status.id(),
                    payload
                );
                let topic = format!("hw5800/{:X}", status.id());
                let msg = mqtt::Message::new(topic, payload, 1);
                if let Err(e) = cli.publish(msg) {
                    println!("Error publishing: {:?}", e);
                    // exit so we can restart and reconnect
                    process::exit(1);
                }
            },
            rtl_num,
        );
    } else {
        // no MQTT server was provided, just print to stdout.
        hw5800(
            |status: &hw5800::HW5800Status| {
                let payload = devs.as_json(status);
                println!("Device: {:02X} status: {}", status.id(), payload);
            },
            rtl_num,
        );
    }
}
