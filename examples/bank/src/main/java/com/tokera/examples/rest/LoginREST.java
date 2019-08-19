package com.tokera.examples.rest;

import com.tokera.ate.delegates.AteDelegate;
import com.tokera.ate.dto.TokenDto;
import com.tokera.ate.dto.msg.MessagePrivateKeyDto;
import com.tokera.ate.security.TokenBuilder;
import com.tokera.examples.dto.PasswordLoginRequest;
import com.tokera.examples.dto.RootLoginRequest;

import javax.annotation.security.PermitAll;
import javax.enterprise.context.RequestScoped;
import javax.ws.rs.Consumes;
import javax.ws.rs.POST;
import javax.ws.rs.Path;
import javax.ws.rs.Produces;
import javax.ws.rs.core.MediaType;

@RequestScoped
@Path("/login")
public class LoginREST {
    protected AteDelegate d = AteDelegate.get();

    @POST
    @Path("/root")
    @Produces(MediaType.APPLICATION_XML)
    @Consumes({"text/yaml", MediaType.APPLICATION_JSON})
    @PermitAll
    public String rootLogin(RootLoginRequest request) {
        return new TokenBuilder()
                .withUsername(request.getUsername())
                .addReadKeys(request.getReadRights())
                .addWriteKeys(request.getWriteRights())
                .shouldPublish(true)
                .build()
                .getXmlToken();
    }

    @POST
    @Path("/password")
    @Produces(MediaType.APPLICATION_XML)
    @Consumes({"text/yaml", MediaType.APPLICATION_JSON})
    @PermitAll
    public String passwordLogin(PasswordLoginRequest request) {
        MessagePrivateKeyDto writeKey = d.encryptor.genSignKeyFromSeedWithAlias(256, request.getPasswordHash(), request.getUsername());
        MessagePrivateKeyDto readKey = d.encryptor.genEncryptKeyFromSeedWithAlias(256, request.getPasswordHash(), request.getUsername());
        return new TokenBuilder()
                .withUsername(request.getUsername())
                .addReadKey(readKey)
                .addWriteKey(writeKey)
                .shouldPublish(true)
                .build()
                .getXmlToken();
    }

    @POST
    @Path("/token")
    @Produces(MediaType.TEXT_PLAIN)
    @Consumes(MediaType.APPLICATION_XML)
    @PermitAll
    public String tokenLogin(String tokenTxt) {
        TokenDto token = new TokenDto(tokenTxt);
        d.currentToken.publishToken(token);
        return token.getHash();
    }
}
