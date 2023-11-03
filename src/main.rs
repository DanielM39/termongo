use anyhow::{anyhow, Context, Result};
use clap::Parser;
use color_print::{cprint, cprintln};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute,
    terminal::{self, ClearType},
};
use futures::stream::TryStreamExt;
use mongodb::{options::ClientOptions, Client, Database};
use serde_json::Value;
use std::{io, process};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    connect: String,
}

enum State {
    Default,
    InsideDatabase,
    InsideCollection,
}

const BASE: u32 = 0x0010_ffff + 1;
const BASE_CONTROL: u32 = 0x0200_0000;
const BASE_META: u32 = 0x0400_0000;
const BASE_SHIFT: u32 = 0x0100_0000;
const ESCAPE: u32 = 27;
const PAGE_UP: u32 = BASE + 1;
const PAGE_DOWN: u32 = PAGE_UP + 1;
const DOWN: u32 = PAGE_DOWN + 1;
const UP: u32 = DOWN + 1;
const LEFT: u32 = UP + 1;
const RIGHT: u32 = LEFT + 1;
const HOME: u32 = RIGHT + 1;
const END: u32 = HOME + 1;
const DELETE: u32 = END + 1;
const INSERT: u32 = DELETE + 1;

struct App {
    client: Client,
    state: State,
    list: Vec<(String, usize)>,
    collection_name: String,
    collection_list: Option<Vec<(String, usize)>>,
    database: Option<Database>,
    database_name: String,
    previous_line: usize,
}

impl App {
    async fn change_state(&mut self, state: &State, database: Option<&str>) -> Result<()> {
        terminal::disable_raw_mode()?;
        match state {
            State::Default => {
                print!(
                    "{}{}",
                    cursor::MoveTo(0, 0),
                    terminal::Clear(ClearType::All),
                );
                for item in &self.list {
                    cprintln!("<green>></green>  {}", item.0);
                }
            }
            State::InsideDatabase => {
                print!(
                    "{}{}",
                    cursor::MoveTo(0, 0),
                    terminal::Clear(ClearType::All),
                );

                let name = database.unwrap();
                cprintln!("<yellow>/{}</yellow>", name);

                let db = self.client.database(name);

                let list: Vec<(_, _)> = db
                    .list_collection_names(None)
                    .await?
                    .into_iter()
                    .enumerate()
                    .map(|(i, x)| (x, i))
                    .collect();

                for collection_name in &list {
                    cprint!("<green>></green>  {}\n", collection_name.0);
                }

                self.collection_list = Some(list);
                self.database = Some(db);
            }
            State::InsideCollection => {
                print!(
                    "{}{}",
                    cursor::MoveTo(0, 0),
                    terminal::Clear(ClearType::All),
                );

                let collection = self
                    .database
                    .as_ref()
                    .unwrap()
                    .collection::<Value>(database.as_ref().expect("No data."));

                let cursor = match collection.find(None, None).await {
                    Ok(cursor) => cursor,
                    Err(_) => return Err(anyhow!("No cursor found.")),
                };

                let data = cursor.try_collect().await.unwrap_or_else(|_| vec![]);

                cprintln!(
                    "<yellow>{}/{}</yellow>",
                    self.database_name,
                    database.unwrap()
                );
                for i in data {
                    println!("{i}");
                }
            }
        }
        execute!(
            io::stdout(),
            cursor::MoveToRow(self.previous_line as u16 + 1)
        )?;
        terminal::enable_raw_mode()?;
        Ok(())
    }
}

async fn connect(connection_string: String) -> Result<Client> {
    let client_options = ClientOptions::parse(connection_string).await;
    match client_options {
        Ok(c) => {
            let client: Client = Client::with_options(c)?;
            Ok(client)
        }
        Err(e) => Err(anyhow!("Invalid connection string: {}", e)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = connect(args.connect).await.unwrap();
    let l = client.list_database_names(None, None).await?;
    let list: Vec<(_, _)> = l.into_iter().enumerate().map(|(i, x)| (x, i)).collect();

    let mut app = App {
        list,
        client,
        state: State::Default,
        collection_name: String::new(),
        collection_list: None,
        database: None,
        database_name: String::from("None"),
        previous_line: 1,
    };

    let mut stdout = io::stdout();
    terminal::enable_raw_mode().context("failed to put terminal in raw mode")?;
    terminal::disable_raw_mode()?;
    print!(
        "{}{}",
        cursor::MoveToRow(0),
        terminal::Clear(ClearType::All),
    );
    for item in &app.list {
        cprintln!("<green>></green>  {}", item.0);
    }

    terminal::enable_raw_mode()?;

    loop {
        if let Event::Key(event) = event::read().context("failed to read a terminal event")? {
            match app.state {
                State::Default => match event.code {
                    KeyCode::ESCAPE => {
                        terminal::disable_raw_mode()?;
                        process::exit(0)
                    }
                    KeyCode::DOWN => execute!(stdout, cursor::MoveDown(1))?,
                    KeyCode::UP => execute!(stdout, cursor::MoveUp(1))?,
                    KeyCode::Enter => {
                        let index = cursor::position()?.1 as usize;
                        for item in &app.list {
                            if item.1 == index {
                                app.previous_line = index;
                                let matc = String::from(&item.0);
                                app.state = State::InsideDatabase;
                                app.database_name = matc.clone();
                                app.change_state(&State::InsideDatabase, Some(&matc))
                                    .await?;
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                State::InsideDatabase => match event.code {
                    KeyCode::DOWN => execute!(stdout, cursor::MoveDown(1))?,
                    KeyCode::UP => execute!(stdout, cursor::MoveUp(1))?,
                    KeyCode::ESCAPE => {
                        app.state = State::Default;
                        app.change_state(&State::Default, Some(&String::from("none")))
                            .await?;
                    }
                    KeyCode::Enter => {
                        let index: usize = (cursor::position()?.1 - 1).into();
                        let collection = app.collection_list.take().expect("No collection found.");
                        for i in &collection {
                            let (item, item_index) = i;
                            if item_index == &index {
                                app.previous_line = index;
                                app.state = State::InsideCollection;
                                app.collection_name = item.to_string();
                                app.change_state(&State::InsideCollection, Some(item))
                                    .await?;
                            }
                        }
                    }
                    _ => {}
                },
                State::InsideCollection => match event.code {
                    KeyCode::DOWN => execute!(stdout, cursor::MoveDown(1))?,
                    KeyCode::UP => execute!(stdout, cursor::MoveUp(1))?,
                    KeyCode::ESCAPE => {
                        app.state = State::InsideDatabase;
                        app.change_state(&State::InsideDatabase, Some(&app.database_name.clone()))
                            .await?;
                    }
                    _ => {}
                },
            }
        }
    }
}
