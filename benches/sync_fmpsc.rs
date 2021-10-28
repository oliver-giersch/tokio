use bencher::{black_box, Bencher};
use tokio::sync::fmpsc;

type Medium = [usize; 64];
type Large = [Medium; 64];

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(6)
        .build()
        .unwrap()
}

fn create_1_medium(b: &mut Bencher) {
    b.iter(|| {
        black_box(&fmpsc::channel::<Medium>(1));
    });
}

fn create_100_medium(b: &mut Bencher) {
    b.iter(|| {
        black_box(&fmpsc::channel::<Medium>(100));
    });
}

fn create_100_000_medium(b: &mut Bencher) {
    b.iter(|| {
        black_box(&fmpsc::channel::<Medium>(100_000));
    });
}

fn send_medium(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        let (mut tx, mut rx) = fmpsc::channel::<Medium>(1000);

        let _ = rt.block_on(tx.send([0; 64]));

        rt.block_on(rx.recv()).unwrap();
    });
}

fn send_large(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        let (mut tx, mut rx) = fmpsc::channel::<Large>(1000);

        let _ = rt.block_on(tx.send([[0; 64]; 64]));

        rt.block_on(rx.recv()).unwrap();
    });
}

fn send_large_boxed(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        let (mut tx, mut rx) = fmpsc::channel::<Box<Large>>(1000);

        let _ = rt.block_on(tx.send(Box::new([[0; 64]; 64])));

        let _r = *rt.block_on(rx.recv()).unwrap();
    });
}

fn contention_bounded(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        rt.block_on(async move {
            let (tx, mut rx) = fmpsc::channel::<usize>(1_000_000);

            for _ in 0..5 {
                let mut tx = tx.clone();
                tokio::spawn(async move {
                    for i in 0..1000 {
                        tx.send(i).await.unwrap();
                    }
                });
            }

            for _ in 0..1_000 * 5 {
                let _ = rx.recv().await;
            }
        })
    });
}

fn contention_bounded_full(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        rt.block_on(async move {
            let (tx, mut rx) = fmpsc::channel::<usize>(100);

            for _ in 0..5 {
                let mut tx = tx.clone();
                tokio::spawn(async move {
                    for i in 0..1000 {
                        tx.send(i).await.unwrap();
                    }
                });
            }

            for _ in 0..1_000 * 5 {
                let _ = rx.recv().await;
            }
        })
    });
}

fn contention_unbounded(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        rt.block_on(async move {
            let (tx, mut rx) = fmpsc::unbounded_channel::<usize>();

            for _ in 0..5 {
                let mut tx = tx.clone();
                tokio::spawn(async move {
                    for i in 0..1000 {
                        tx.send(i).unwrap();
                    }
                });
            }

            for _ in 0..1_000 * 5 {
                let _ = rx.recv().await;
            }
        })
    });
}

fn uncontented_bounded(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        rt.block_on(async move {
            let (mut tx, mut rx) = fmpsc::channel::<usize>(1_000_000);

            for i in 0..5000 {
                tx.send(i).await.unwrap();
            }

            for _ in 0..5_000 {
                let _ = rx.recv().await;
            }
        })
    });
}

fn uncontented_unbounded(b: &mut Bencher) {
    let rt = rt();

    b.iter(|| {
        rt.block_on(async move {
            let (mut tx, mut rx) = fmpsc::unbounded_channel::<usize>();

            for i in 0..5000 {
                tx.send(i).unwrap();
            }

            for _ in 0..5_000 {
                let _ = rx.recv().await;
            }
        })
    });
}

bencher::benchmark_group!(
    create,
    create_1_medium,
    create_100_medium,
    create_100_000_medium
);

bencher::benchmark_group!(send, send_medium, send_large, send_large_boxed);

bencher::benchmark_group!(
    contention,
    contention_bounded,
    contention_bounded_full,
    contention_unbounded,
    uncontented_bounded,
    uncontented_unbounded
);

bencher::benchmark_main!(create, send, contention);
