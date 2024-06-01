

mod config;
mod file_help;
mod list;
mod main_pplm;
mod main_rules;
mod map;

#[tokio::main]
async fn main()
{
	if true
	{
		main_pplm::main().await;
	}
	else
	{
		main_rules::main().await;
	}
}
