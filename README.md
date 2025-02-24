Library to parse/serialize UDP punch requests, and track requesters.

There is no wrapping of a UDP socket in this library, it is assumed you have your own system and can filter
directly the udpunch messages from your framework.

All udpunch messages start with "#punch@". Every UDP that has those 6 characters can be considered to be owned by this
library.

More precisely each message has this structure `#punch@$REQUEST_ID/key1=value1/key2=value2/key3=value3`.

For instance, a request to connect message would be like this:

`#punch@register/from=steamid:123456789`.

Three messages types are used in this library

* Register: register an ID with this crate, that can be communicated to someone else (along with ip:port).
* Request: request to join an ID, first passed from client1 to middleman, then to middleman to host.
* PunchOrder: order to punch with UDP a specific remote, to connect to a specific person.