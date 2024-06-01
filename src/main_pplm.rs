
pub async fn main()
{ 
    println!("test");
    loop {}
    /*let mut last_start_time = SystemTime::now();
    let mut fail_count = 0;
    loop {
        let start_time = SystemTime::now();
        match connect_run().await {
            Ok(_) => {}
            Err(e) => {
                println!("error in main {:?}", e);
            }
        }
        if start_time.duration_since(last_start_time).unwrap() < Duration::from_secs( 60 * 60 ) {
            fail_count += 1;
        } else {
            fail_count = 0;
        }
        let sleep_duration = Duration::from_secs(2u64.pow(fail_count));
        println!("disconnected, reconnecting in {:?}s", sleep_duration);
        std::thread::sleep(sleep_duration);
        last_start_time = start_time;
    }
    */
}
