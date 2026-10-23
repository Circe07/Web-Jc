pub struct Request {
    pub id: String,
}

pub fn handle(req: &Request) -> u64 {
    let id: u64 = req.id.clone();
    id
}
