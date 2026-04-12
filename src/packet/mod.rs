use pnet::packet::Packet;
use pnet::packet::ip::IpNextHeaderProtocols;
use pnet::packet::ipv4::Ipv4Packet;
use pnet::packet::tcp::TcpPacket;
use pnet::packet::udp::UdpPacket;

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Tcp,
    Udp,
    Other(u8),
}

#[derive(Debug, Clone)]
pub struct IpPacket {
    pub src_ip: std::net::IpAddr,
    pub dst_ip: std::net::IpAddr,
    pub protocol: Protocol,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub payload: Vec<u8>,
    pub raw: Vec<u8>,
}

impl IpPacket {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let ipv4 = Ipv4Packet::new(data)?;
        
        let src_ip = std::net::IpAddr::V4(ipv4.get_source());
        let dst_ip = std::net::IpAddr::V4(ipv4.get_destination());
        
        let ipv4_payload = ipv4.payload();
        let (protocol, src_port, dst_port, payload) = match ipv4.get_next_level_protocol() {
            IpNextHeaderProtocols::Tcp => {
                if let Some(tcp) = TcpPacket::new(ipv4_payload) {
                    let payload = tcp.payload().to_vec();
                    (
                        Protocol::Tcp,
                        Some(tcp.get_source()),
                        Some(tcp.get_destination()),
                        payload,
                    )
                } else {
                    return None;
                }
            }
            IpNextHeaderProtocols::Udp => {
                if let Some(udp) = UdpPacket::new(ipv4_payload) {
                    let payload = udp.payload().to_vec();
                    (
                        Protocol::Udp,
                        Some(udp.get_source()),
                        Some(udp.get_destination()),
                        payload,
                    )
                } else {
                    return None;
                }
            }
            proto => (Protocol::Other(proto.0), None, None, ipv4_payload.to_vec()),
        };
        
        Some(IpPacket {
            src_ip,
            dst_ip,
            protocol,
            src_port,
            dst_port,
            payload,
            raw: data.to_vec(),
        })
    }
    
    pub fn connection_key(&self) -> String {
        match self.protocol {
            Protocol::Tcp | Protocol::Udp => {
                format!("{}:{}->{}:{}", 
                    self.src_ip, self.src_port.unwrap_or(0),
                    self.dst_ip, self.dst_port.unwrap_or(0)
                )
            }
            _ => format!("{}->{}",  self.src_ip, self.dst_ip),
        }
    }
}
