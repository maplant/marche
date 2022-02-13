$(document).ready(function () {
    var easymde = new EasyMDE({ status: false, lineWrapping: true, });
    $('.editor-toolbar').css('border', 'none');

    $(".reply-to-button").click(function () {
        $("#reply")[0].scrollIntoView({ behavior: "smooth" });
        insertAtCurrentLine(easymde, ` @respond:${$(this).attr('replyid')}`);
    });

    $(".respond-to-preview").click(function () {
        getReplyDiv($(this))[0].scrollIntoView({ behavior: "smooth", block: "center" });
    });

    $(".respond-to-preview").hover(function () {
        var overlay_div = getOverlayDiv($(this));

        var reply_div_clone = getReplyDiv($(this)).clone();
        reply_div_clone.removeAttr("id");
        overlay_div[0].replaceChildren(reply_div_clone[0]);
        overlay_div.css("visibility", "visible").css("opacity", "1.0");
    }, function () {
        getOverlayDiv($(this)).css("visibility", "hidden").css("opacity", "0.0");
    });

    // Populate response elements
    $("div.respond-to-preview").each(function () {
        var response_div = $(this).parents(".reply")
        var response_div_clone = response_div.clone().removeAttr("id");
        var responder_name = response_div.attr("author");
        var response_preview_div = $($.parseHTML(`<div class="response-from-preview action-box" reply_id="{reply_id}"><b>^${responder_name}</b></div>`));
        var response_overlay_div = $($.parseHTML(`<div class="response-from-preview response-overlay overlay-on-hover" style="display: inline-block;"></div>`));
        response_preview_div.hover(function () {
            response_overlay_div[0].replaceChildren((response_div_clone[0]));
            response_overlay_div.css("visibility", "visible").css("opacity", "1.0");
        },
            function () {
                response_overlay_div.css("visibility", "hidden").css("opacity", "0.0");
            })
        response_preview_div.click(function () {
            response_div[0].scrollIntoView({ behavior: "smooth", block: "center" });
        })
        response_overlay_div[0].appendChild(response_div_clone[0]);
        getResponseContainerDiv($(this)).append(response_preview_div).append(response_overlay_div);
    });
});

function insertAtCurrentLine(mde, text) {
    cm = mde.codemirror;
    var doc = cm.getDoc();
    var cursor = doc.getCursor();
    var line = doc.getLine(cursor.line);
    var pos = {
        line: cursor.line,
        ch: line.length
    }
    doc.replaceRange(text, pos);
}

function getOverlayDiv(origin) {
    var overlay_div = origin.next("div.reply-overlay");

    // Workaround because markdown parser doesn't close its own <p> tags.  garbage.
    if (overlay_div.length == 0) {
        overlay_div = origin.parent().next("div.reply-overlay");
    }
    return overlay_div;
}

function getReplyDiv(origin) {
    // If we ever add paginated threads, this logic needs to be extended in order to retrieve/render replys which are not in the DOM
    return $(`#${origin.attr("reply_id")}`);
}

function getResponseContainerDiv(origin) {
    // If we ever add paginated threads, this logic needs to be extended in order to retrieve/render replys which are not in the DOM
    return $(`#response-container-${origin.attr("reply_id")}`);
}
