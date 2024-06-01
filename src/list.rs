
pub fn parse_list<'a>(contents: &'a str) -> Vec<&'a str> {
    let mut data = Vec::new();
    for line in contents.lines() {
        if line.len() == 0 { continue; }
        if let Some('-') = line.chars().next() {
            continue;
        }
        data.push(line); 
    } 
    data
}
