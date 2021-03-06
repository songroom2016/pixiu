mod quote_event;
mod quotespi;
mod trader_event;
mod traderspi;

use self::quote_event::QuoteEvent;
use self::quotespi::QSpi;
use self::trader_event::TraderEvent;
use self::traderspi::TSpi;
use crate::{Exchange, Strategy};
use async_trait::async_trait;
use failure::Fallible;
use futures::stream::StreamExt;
use std::net::SocketAddrV4;
use std::sync::Arc;
use tokio::select;
use tokio::sync::{broadcast, mpsc};
use xtp::{QuoteApi, TraderApi, XTPExchangeType, XTPLogLevel, XTPProtocolType};

pub struct XTPExchange {
    quote_addr: SocketAddrV4,
    trader_addr: SocketAddrV4,
    username: String,
    password: String,
    key: String,
    strategies: Vec<Box<dyn Strategy<XTPExchange> + Send + Sync>>,

    quote_api: Option<Arc<QuoteApi>>,
    trader_api: Option<Arc<TraderApi>>,

    quote_rx: Option<mpsc::Receiver<QuoteEvent>>,
    trader_rx: Option<mpsc::Receiver<TraderEvent>>,

    strategy_tx: broadcast::Sender<QuoteEvent>,
}

#[derive(Clone)]
pub struct XTPExchangeHandle {
    quote_api: Arc<QuoteApi>,
    trader_api: Arc<TraderApi>,
}

impl XTPExchangeHandle {
    fn new(quote_api: Arc<QuoteApi>, trader_api: Arc<TraderApi>) -> Self {
        Self {
            quote_api,
            trader_api,
        }
    }

    pub fn subscribe_market_data(
        &self,
        tickers: &[&str],
        exchange_id: XTPExchangeType,
    ) -> Fallible<()> {
        self.quote_api.subscribe_market_data(tickers, exchange_id)
    }
}

impl XTPExchange {
    pub fn new(
        quote_addr: SocketAddrV4,
        trader_addr: SocketAddrV4,
        username: &str,
        password: &str,
        key: &str,
    ) -> XTPExchange {
        let (tx, _) = broadcast::channel(10);

        XTPExchange {
            quote_addr,
            trader_addr,
            username: username.to_string(),
            password: password.to_string(),
            key: key.to_string(),
            strategies: vec![],
            quote_api: None,
            trader_api: None,
            quote_rx: None,
            trader_rx: None,
            strategy_tx: tx,
        }
    }

    fn sys_init(&mut self) {
        let mut qapi = QuoteApi::new(1, "/tmp/xtp", XTPLogLevel::Trace);
        let (tx, rx) = mpsc::channel(10);
        qapi.register_spi(QSpi::new(tx));
        qapi.set_heart_beat_interval(10);
        qapi.set_udp_buffer_size(1024);
        qapi.login(
            self.quote_addr,
            &self.username,
            &self.password,
            XTPProtocolType::TCP,
        )
        .unwrap();

        self.quote_api = Some(Arc::new(qapi));
        self.quote_rx = Some(rx);

        let mut tapi = TraderApi::new(1, "/tmp/xtp", XTPLogLevel::Trace);
        let (tx, rx) = mpsc::channel(10);
        tapi.register_spi(TSpi::new(tx));
        tapi.set_heart_beat_interval(10);
        tapi.set_software_key(&self.key).unwrap(); // MUST SET KEY FIRST! BEFORE LOGIN
        tapi.login(
            self.trader_addr,
            &self.username,
            &self.password,
            XTPProtocolType::TCP,
        )
        .unwrap();

        self.trader_api = Some(Arc::new(tapi));
        self.trader_rx = Some(rx);
    }

    fn handle(&self) -> XTPExchangeHandle {
        XTPExchangeHandle::new(
            self.quote_api.clone().unwrap(),
            self.trader_api.clone().unwrap(),
        )
    }
}

#[async_trait]
impl Exchange for XTPExchange {
    type Event = QuoteEvent;
    type Handle = XTPExchangeHandle;

    async fn run(mut self) {
        self.sys_init();
        let h = self.handle();

        for s in self.strategies {
            tokio::spawn(s.run(self.strategy_tx.subscribe(), h.clone()));
        }

        let mut qrx = self.quote_rx.unwrap();
        let mut trx = self.trader_rx.unwrap();
        let stx = self.strategy_tx;

        loop {
            select! {
                Some(msg) = qrx.next() => {
                    stx.send(msg);
                }
                _ = trx.next() => {}
            }
        }
    }

    fn register<S>(&mut self, s: S)
    where
        S: Strategy<XTPExchange> + Send + Sync + 'static,
    {
        self.strategies.push(Box::new(s))
    }
}
